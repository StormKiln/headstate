//! Opt-in synthetic native acceptance driver. Never called by ordinary app setup.
//! Product calls use real handlers; paired calls traverse the real pinned mTLS listener.
pub(crate) mod metrics;
mod phone;
use crate::{commands, github::client::GitHubClient, poll, remote, store};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{Assets, Listener, Manager, Wry};
use tokio::sync::{broadcast, Notify};

pub(crate) struct Profile(pub PathBuf);
static WORKERS: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>> = Mutex::new(Vec::new());
pub(crate) fn track_worker(task: tauri::async_runtime::JoinHandle<()>) {
    WORKERS.lock().unwrap().push(task);
}
#[derive(Deserialize)]
struct Config {
    profile: PathBuf,
    provider: String,
    bridge_secret: String,
    ready: PathBuf,
}
struct Empty;
impl Assets<Wry> for Empty {
    fn get(&self, _: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }
    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }
    fn csp_hashes(&self, _: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}
#[derive(Clone, Serialize)]
struct Event {
    name: String,
    payload: Value,
}
struct Driver {
    app: tauri::AppHandle,
    secret: String,
    addr: SocketAddr,
    server_fp: String,
    phone: remote::identity::Identity,
    pairing: Arc<remote::pairing::PairingState>,
    desktop_events: broadcast::Sender<Event>,
    paired_events: broadcast::Sender<Event>,
    paired_closed: AtomicBool,
    lost: AtomicBool,
    started: AtomicBool,
    stop: Notify,
    pairing_requests: tokio::sync::Mutex<
        tokio::sync::mpsc::UnboundedReceiver<remote::pairing::PairingRequestEvent>,
    >,
}
fn authorized(d: &Driver, headers: &HeaderMap) -> bool {
    headers
        .get("x-enterprise-secret")
        .and_then(|h| h.to_str().ok())
        == Some(d.secret.as_str())
}
fn supported(command: &str) -> bool {
    matches!(
        command,
        "get_auth_state"
            | "act_on_pr"
            | "diag_log"
            | "get_reviewing"
            | "get_viewer"
            | "get_ready_stacks"
            | "get_ready_pushers"
            | "get_review_gates"
            | "review_pr_at_head"
            | "get_ui_prefs"
            | "claude_sessions_for_pr_number"
            | "stats_count"
            | "stats_series"
            | "stats_reviewers"
            | "get_source_snapshot"
            | "get_cached"
            | "get_cached_reviewing"
            | "refresh_source"
            | "refresh_now"
            | "get_pr_detail"
            | "get_pr_checks"
            | "get_pr_reviews"
            | "get_review_threads"
            | "stats_board"
            | "stats_board_cached"
            | "stats_tree"
            | "stats_backfill"
            | "stats_demand"
            | "get_pr_review_gate"
            | "approve_pr"
            | "submit_review"
            | "get_review_comments"
            | "get_pr_diff"
            | "get_pr_files"
            | "get_pr_advisory"
            | "get_poll_interval"
    )
}
async fn call(
    State(d): State<Arc<Driver>>,
    Path(route): Path<(String, String)>,
    headers: HeaderMap,
    Json(args): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&d, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    let mut scope = metrics::Scope::new("command", u64::from(route.0 == "paired"));
    let (status, Json(mut value)) = metrics::COMMAND
        .scope(
            scope.id(),
            call_inner(State(d), Path(route), headers, Json(args)),
        )
        .await;
    scope.finish(if status.is_success() {
        "complete"
    } else {
        "refused"
    });
    if let Some(object) = value.as_object_mut() {
        object.insert("callId".into(), json!(scope.id()));
    }
    (status, Json(value))
}
async fn call_inner(
    State(d): State<Arc<Driver>>,
    Path((role, command)): Path<(String, String)>,
    headers: HeaderMap,
    Json(args): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&d, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    if !supported(&command) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"unsupported harness command"})),
        );
    }
    if role == "desktop" && command == "diag_log" {
        commands::diag_log(
            args.get("line")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
        return (StatusCode::OK, Json(json!({"value":null})));
    }
    if role == "desktop" {
        match remote::surface::dispatch(
            &d.app,
            &command,
            args,
            &remote::context::DispatchContext::desktop(),
        )
        .await
        {
            Ok(value) => (StatusCode::OK, Json(json!({"value":value}))),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            ),
        }
    } else if role == "paired" {
        let body = serde_json::to_string(&args).unwrap();
        match phone::request(
            d.addr,
            Some(&d.phone),
            &d.server_fp,
            "POST",
            &format!("/v1/call/{command}"),
            &[("Content-Type", "application/json")],
            Some(&body),
        )
        .await
        {
            Ok(reply) => (
                StatusCode::from_u16(reply.status).unwrap(),
                Json(
                    json!({"wire":serde_json::from_str::<Value>(&reply.body).unwrap_or(Value::Null)}),
                ),
            ),
            Err(_) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error":"paired transport closed"})),
            ),
        }
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"unknown role"})),
        )
    }
}
async fn events(
    State(d): State<Arc<Driver>>,
    Path(role): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    use axum::response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse,
    };
    if !authorized(&d, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let rx = match role.as_str() {
        "desktop" => d.desktop_events.subscribe(),
        "paired" => d.paired_events.subscribe(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let stream = futures_util::stream::unfold((rx, d, role), |(mut rx, d, role)| async move {
        if role == "paired" && d.paired_closed.load(Ordering::SeqCst) {
            return None;
        }
        match rx.recv().await {
            Ok(event) => Some((
                Ok::<_, std::convert::Infallible>(
                    SseEvent::default()
                        .event(event.name)
                        .data(event.payload.to_string()),
                ),
                (rx, d, role),
            )),
            Err(_) => {
                d.lost.store(true, Ordering::SeqCst);
                None
            }
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
async fn control(
    State(d): State<Arc<Driver>>,
    Path(action): Path<String>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if !authorized(&d, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    match action.as_str() {
        "start" => {
            if !d.started.swap(true, Ordering::SeqCst) {
                let app = &d.app;
                let client = app
                    .state::<commands::GhClient>()
                    .0
                    .as_ref()
                    .unwrap()
                    .clone();
                poll::spawn(
                    app.clone(),
                    client.clone(),
                    app.state::<crate::Focused>().0.clone(),
                    app.state::<poll::Waker>().0.clone(),
                    app.state::<poll::PollInterval>().0.clone(),
                    app.state::<poll::ViewNeedsGithub>().0.clone(),
                    app.state::<poll::GithubSourceEnabled>().0.clone(),
                );
                poll::spawn_backfill(
                    app.clone(),
                    client,
                    app.state::<poll::BackfillWaker>().0.clone(),
                );
            }
        }
        "revoke" => {
            let conn = store::open_db(&commands::db_path(&d.app)).unwrap();
            if let Some(device) = d.pairing.paired_device(&d.phone.fingerprint()) {
                d.pairing.revoke(&conn, device.id).unwrap();
            }
        }
        "repair" => {
            repair(&d).await.expect("synthetic re-pair");
            let mut stream = phone::SseClient::connect(d.addr, &d.phone, &d.server_fp).await;
            assert_eq!(stream.status, 200);
            d.paired_closed.store(false, Ordering::SeqCst);
            let events_driver = d.clone();
            track_worker(tauri::async_runtime::spawn(async move {
                while let Some((name, data)) = stream.next_frame().await {
                    if let Ok(payload) = serde_json::from_str(&data) {
                        let _ = events_driver.paired_events.send(Event { name, payload });
                    }
                }
                events_driver.paired_closed.store(true, Ordering::SeqCst);
            }));
        }
        "stop" => {
            metrics::release_commit_hold();
            d.stop.notify_one();
        }
        "wake" => d.app.state::<poll::Waker>().0.notify_one(),
        "hold-commit" => metrics::arm_commit_hold(),
        "release-commit" => metrics::release_commit_hold(),
        "retire-account" => {
            let conn = store::open_db(&commands::db_path(&d.app)).unwrap();
            store::stats_owner::capture_verified(&conn, "synthetic-replacement").unwrap();
        }
        "restore-account" => {
            let conn = store::open_db(&commands::db_path(&d.app)).unwrap();
            store::stats_owner::capture_verified(&conn, "synthetic-viewer").unwrap();
        }
        "fail-queue-write" | "recover-queue-write" => {
            let conn = store::open_db(&commands::db_path(&d.app)).unwrap();
            if action == "fail-queue-write" {
                conn.execute_batch("CREATE TRIGGER enterprise_queue_failure BEFORE UPDATE ON queue_scan BEGIN SELECT RAISE(ABORT,'synthetic queue transaction failure'); END;").unwrap();
            } else {
                conn.execute_batch("DROP TRIGGER IF EXISTS enterprise_queue_failure;")
                    .unwrap();
            }
        }
        "status" => {}
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"unknown control"})),
            )
        }
    }
    (
        StatusCode::OK,
        Json(
            json!({"started":d.started.load(Ordering::SeqCst),"pairedClosed":d.paired_closed.load(Ordering::SeqCst),"commitHeld":metrics::commit_held(),"admission":d.app.state::<commands::GhClient>().0.as_ref().map(|client|client.admission_snapshot()),"telemetryLost":d.lost.load(Ordering::SeqCst) || metrics::lost()}),
        ),
    )
}
async fn repair(d: &Arc<Driver>) -> Result<(), String> {
    let ecdsa = p256::ecdsa::SigningKey::from_bytes(&[7u8; 32].into()).unwrap();
    let mldsa = ml_dsa::SigningKey::<ml_dsa::MlDsa65>::from_seed(&ml_dsa::Seed::from([7u8; 32]));
    let issued = d.pairing.issue_token();
    let token = URL_SAFE_NO_PAD.decode(&issued.token).unwrap();
    let req = remote::pairing::PairRequest {
        token: issued.token,
        device_name: "synthetic-consumer".into(),
        signing_keys: remote::pairing::SigningKeys {
            ecdsa_p256: STANDARD.encode(ecdsa.verifying_key().to_sec1_point(false).as_bytes()),
            mldsa_65: Some(STANDARD.encode(mldsa.expanded_key().verifying_key().encode())),
        },
        proof: remote::pairing::proof(&token, &d.phone.fingerprint(), &d.server_fp),
    };
    let body = serde_json::to_string(&req).unwrap();
    let driver = d.clone();
    let pending = tokio::spawn(async move {
        phone::request(
            driver.addr,
            Some(&driver.phone),
            &driver.server_fp,
            "POST",
            "/v1/pair",
            &[("Content-Type", "application/json")],
            Some(&body),
        )
        .await
    });
    let mut requests = d.pairing_requests.lock().await;
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), requests.recv())
        .await
        .map_err(|_| "repair event timeout")?
        .ok_or("repair event closed")?;
    let conn = store::open_db(&commands::db_path(&d.app)).map_err(|e| e.to_string())?;
    d.pairing
        .respond(
            &conn,
            event.request_id,
            remote::pairing::PairDecision::Approve {
                same_name: remote::pairing::SameName::KeepBoth,
            },
        )
        .map_err(|e| e.to_string())?;
    drop(conn);
    let result = pending.await.map_err(|_| "repair task")??;
    if result.status != 200 {
        return Err("repair refused".into());
    }
    Ok(())
}
async fn setup(app: tauri::AppHandle, config: Config) -> Result<(), String> {
    let (tx, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let pairing = Arc::new(remote::pairing::PairingState::new(move |event| {
        let _ = tx.send(event);
    }));
    let conn = store::open_db(&commands::db_path(&app)).map_err(|e| e.to_string())?;
    pairing.reload_devices(&conn).map_err(|e| e.to_string())?;
    drop(conn);
    app.manage(pairing.clone());
    let identity = remote::identity::Identity::generate().map_err(|e| e.to_string())?;
    let server_fp = identity.fingerprint();
    let (listener, _hub) = remote::gate::start_synthetic(&app, identity, pairing.clone()).await?;
    let addr = listener.local_addr();
    let phone = remote::identity::Identity::generate().map_err(|e| e.to_string())?;
    let ecdsa = p256::ecdsa::SigningKey::from_bytes(&[7u8; 32].into()).unwrap();
    let mldsa = ml_dsa::SigningKey::<ml_dsa::MlDsa65>::from_seed(&ml_dsa::Seed::from([7u8; 32]));
    let issued = pairing.issue_token();
    let token = URL_SAFE_NO_PAD.decode(&issued.token).unwrap();
    let req = remote::pairing::PairRequest {
        token: issued.token,
        device_name: "synthetic-consumer".into(),
        signing_keys: remote::pairing::SigningKeys {
            ecdsa_p256: STANDARD.encode(ecdsa.verifying_key().to_sec1_point(false).as_bytes()),
            mldsa_65: Some(STANDARD.encode(mldsa.expanded_key().verifying_key().encode())),
        },
        proof: remote::pairing::proof(&token, &phone.fingerprint(), &server_fp),
    };
    let body = serde_json::to_string(&req).unwrap();
    let cert = phone.clone();
    let fp = server_fp.clone();
    let pending = tokio::spawn(async move {
        phone::request(
            addr,
            Some(&cert),
            &fp,
            "POST",
            "/v1/pair",
            &[("Content-Type", "application/json")],
            Some(&body),
        )
        .await
    });
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), requests.recv())
        .await
        .map_err(|_| "pairing event timeout")?
        .ok_or("pairing event closed")?;
    let conn = store::open_db(&commands::db_path(&app)).map_err(|e| e.to_string())?;
    pairing
        .respond(
            &conn,
            event.request_id,
            remote::pairing::PairDecision::Approve {
                same_name: remote::pairing::SameName::KeepBoth,
            },
        )
        .map_err(|e| e.to_string())?;
    drop(conn);
    let paired = pending.await.map_err(|_| "pair task")??;
    assert_eq!(paired.kx, Some(rustls::NamedGroup::X25519MLKEM768));
    if paired.status != 200 {
        return Err(format!("pair status {}", paired.status));
    }
    let mut stream = phone::SseClient::connect(addr, &phone, &server_fp).await;
    if stream.status != 200 || !stream.content_type.starts_with("text/event-stream") {
        return Err("paired event status".into());
    }
    let (desktop_events, _) = broadcast::channel(4096);
    let (paired_events, _) = broadcast::channel(4096);
    let d = Arc::new(Driver {
        app: app.clone(),
        secret: config.bridge_secret,
        addr,
        server_fp,
        phone,
        pairing,
        desktop_events,
        paired_events,
        paired_closed: AtomicBool::new(false),
        lost: AtomicBool::new(false),
        started: AtomicBool::new(false),
        stop: Notify::new(),
        pairing_requests: tokio::sync::Mutex::new(requests),
    });
    for name in remote::events::EVENT_NAMES {
        let d = d.clone();
        let name = name.to_string();
        app.listen_any(name.clone(), move |event| {
            let payload = serde_json::from_str(event.payload()).unwrap_or(Value::Null);
            let _ = d.desktop_events.send(Event {
                name: name.clone(),
                payload,
            });
        });
    }
    let events_driver = d.clone();
    let paired_task = tokio::spawn(async move {
        while let Some((name, data)) = stream.next_frame().await {
            if let Ok(payload) = serde_json::from_str(&data) {
                let _ = events_driver.paired_events.send(Event { name, payload });
            }
        }
        events_driver.paired_closed.store(true, Ordering::SeqCst);
        let _ = events_driver.paired_events.send(Event {
            name: "harness-paired-closed".into(),
            payload: Value::Null,
        });
    });
    let router = Router::new()
        .route("/call/{role}/{command}", post(call))
        .route("/events/{role}", get(events))
        .route("/control/{action}", post(control))
        .with_state(d.clone());
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    std::fs::write(&config.ready,json!({"bridge":format!("http://{}",tcp.local_addr().unwrap()),"paired":true,"native":true}).to_string()).map_err(|e|e.to_string())?;
    let server = tokio::spawn(async move { axum::serve(tcp, router).await });
    d.stop.notified().await;
    server.abort();
    let _ = server.await;
    app.state::<poll::GithubSourceEnabled>()
        .0
        .store(false, Ordering::SeqCst);
    listener.stop().await;
    paired_task.abort();
    let _ = paired_task.await;
    let workers = std::mem::take(&mut *WORKERS.lock().unwrap());
    for worker in &workers {
        worker.abort();
    }
    for worker in workers {
        let _ = worker.await;
    }
    metrics::finish();
    std::fs::write(
        config.profile.join("shutdown.json"),
        json!({"clean":true,"telemetryLost":d.lost.load(Ordering::SeqCst) || metrics::lost()})
            .to_string(),
    )
    .map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}
pub fn run() {
    remote::gate::install_crypto_provider();
    let input = std::env::args().nth(1).expect("config path required");
    let config: Config =
        serde_json::from_slice(&std::fs::read(input).expect("read config")).expect("config JSON");
    assert!(config.profile.is_absolute() && config.ready.is_absolute());
    assert!(config.bridge_secret.len() >= 32);
    let url = reqwest::Url::parse(&config.provider).expect("provider URL");
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    std::fs::create_dir_all(&config.profile).unwrap();
    let marker = config.profile.join(".enterprise-synthetic");
    if config.profile.join("headstate.db").exists() {
        assert!(marker.exists(), "refuse non-synthetic existing database");
    }
    std::fs::write(marker, b"synthetic profile v1").unwrap();
    metrics::set_profile(&config.profile);
    metrics::start(&config.profile.join("native.ndjson")).expect("metrics recorder");
    let db = config.profile.join("headstate.db");
    let conn = store::open_db(&db).unwrap();
    let prefs = poll::NotifyPrefs {
        enabled: false,
        ..Default::default()
    };
    store::settings::set(&conn, store::settings::keys::NOTIFY_PREFS, &prefs).unwrap();
    drop(conn);
    let crab = tauri::async_runtime::block_on(async {
        octocrab::Octocrab::builder()
            .base_uri(&config.provider)
            .unwrap()
            .personal_token("synthetic-enterprise-credential")
            .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
            .build()
            .unwrap()
    });
    let client =
        Arc::new(GitHubClient::new(crab).with_credential("synthetic-enterprise-credential", db));
    let mut tc = tauri::utils::config::Config {
        identifier: "invalid.synthetic.enterprise".into(),
        product_name: Some("Synthetic enterprise driver".into()),
        ..Default::default()
    };
    tc.app.windows.clear();
    let context = tauri::Context::<Wry>::new(
        tc,
        Box::new(Empty),
        None,
        None,
        tauri::PackageInfo {
            name: "enterprise-driver".into(),
            version: "9.0.0".parse().unwrap(),
            authors: "synthetic",
            description: "test driver",
            crate_name: "enterprise_driver",
        },
        tauri::Pattern::Brownfield,
        tauri::ipc::RuntimeAuthority::new(Default::default(), Default::default()),
        None,
    );
    let app = tauri::Builder::<Wry>::default()
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Prohibited);
            app.manage(Profile(config.profile.clone()));
            app.manage(commands::AuthState {
                ok: true,
                message: "synthetic account".into(),
            });
            app.manage(commands::GhClient(Some(client)));
            app.manage(crate::source_poll::SourcePolls::default());
            app.manage(crate::Focused(Arc::new(AtomicBool::new(true))));
            app.manage(poll::Waker(Arc::new(Notify::new())));
            app.manage(poll::BackfillWaker(Arc::new(Notify::new())));
            app.manage(poll::PollInterval(Arc::new(AtomicU64::new(60))));
            app.manage(poll::ViewNeedsGithub(Arc::new(AtomicBool::new(true))));
            app.manage(poll::GithubSourceEnabled(Arc::new(AtomicBool::new(true))));
            app.manage(Arc::new(crate::stats_demand::Registry::default()));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = setup(handle.clone(), config).await {
                    eprintln!("HARNESS_SETUP_FAILED: {e}");
                    handle.exit(2);
                }
            });
            Ok(())
        })
        .build(context)
        .expect("build Wry driver");
    app.run(|_, _| {});
}
