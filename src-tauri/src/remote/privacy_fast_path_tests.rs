use super::*;
use sha2::{Digest, Sha256};

// One witness for each ordered shape, including the deliberately short GitHub token.
const WITNESSES: [&str; 16] = [
    "-----BEGIN RSA PRIVATE KEY-----\r\nsynthetic truncated",
    "ghp_a",
    "sk-abcdefghijklmnopqrst",
    "ASIAABCDEFGHIJKLMNOP",
    "AIzaabcdefghijklmnopqrstuvwxyzABCDEFGHI",
    "rk_test_abcdefghijklmnop",
    "xoxb-1234567890",
    "glpat-abcdefghijklmnopqrst",
    "npm_abcdefghijklmnopqrstuvwxyz0123456789",
    "eyJabcdefghijk.eyJabcdefghijk.abcdefghijkl",
    "aUtHoRiZaTiOn: Basic abcdefgh",
    "bEaReR abcdefghijklmnop1",
    "postgres://user:synthetic@host",
    "API_KEY=synthetic",
    "\"token\": \"synthetic\"",
    "password: synthetic",
];

#[test]
fn prefilter_preserves_each_ordered_shape_and_original_output() {
    assert_eq!(
        SHAPES.len(),
        WITNESSES.len(),
        "new shapes need a trigger witness"
    );
    for (shape, text) in SHAPES.iter().zip(WITNESSES) {
        assert!(
            shape.re.is_match(text),
            "invalid witness for {}",
            shape.re.as_str()
        );
        assert!(may_contain_secret(text), "missed {}", shape.kind);
        assert_eq!(mask_text(text), mask_text_regex(text));
        assert!(mask_text(text).1 > 0);
    }
}

#[test]
fn triggerless_prose_is_admitted_to_the_borrowed_fast_path() {
    for text in [
        "",
        "Ordinary prose with Unicode café 日本語 🦀 and CRLF\r\nnext line.",
        "Review this change (tests passed); keep the original result! See /tmp/example.",
    ] {
        assert!(!may_contain_secret(text));
        assert_eq!(mask_text_regex(text).1, 0);
        assert!(matches!(mask_text(text), (Cow::Borrowed(_), 0)));
    }
}

#[test]
fn generated_mixed_unicode_crlf_overlap_and_placeholders_match_original() {
    let fragments = WITNESSES
        .into_iter()
        .chain([
            "API_KEY=sk-abcdefghijklmnopqrst",
            "PASSWORD=$PASSWORD",
            "secret: true",
            "Bearer authentication",
            "bearer abcdefghijklmnop",
            "AKIAABCDEFGHIJKLMNOP",
            "github_pat_a",
            "gho_a",
            "ghu_a",
            "ghs_a",
            "ghr_a",
            "sk_live_abcdefghijklmnop",
            "pk_test_abcdefghijklmnop",
            "-----BEGIN PRIVATE KEY-----",
            "éghp_a",
            "\"password\":\"ghp_a\"",
            "token: '\" ghp_a",
            "API_KEY=''",
        ])
        .collect::<Vec<_>>();
    let surrounds = ["", " café ", "日本語", "\r\n", "[]{}();", "_", "\u{2003}"];
    let mut state = 1531_u64;
    for i in 0..4096 {
        let mut text = surrounds[i % surrounds.len()].to_owned();
        for _ in 0..1 + i % 7 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            text.push_str(fragments[(state >> 32) as usize % fragments.len()]);
            text.push_str(surrounds[(state >> 16) as usize % surrounds.len()]);
        }
        assert_eq!(mask_text(&text), mask_text_regex(&text), "case {i}");
    }
}

#[test]
fn ordered_regex_policy_changes_require_trigger_review() {
    let mut hash = Sha256::new();
    for shape in SHAPES.iter() {
        hash.update(shape.kind);
        hash.update([0]);
        hash.update(shape.re.as_str());
        hash.update([0]);
    }
    let fingerprint = hash
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        fingerprint, "d8889ec3a74acded21d45f7af84a5f03ebf861a90568b58c9563675f44fc91fd",
        "review all trigger assumptions when policy changes"
    );
}

fn digest(text: &[u8]) -> String {
    Sha256::digest(text)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Explicit, synthetic, same-process comparison; never a timing gate in CI.
#[test]
#[ignore]
fn masking_reference_benchmark() {
    use std::{hint::black_box, time::Instant};
    let Some(out) = std::env::var_os("HEADSTATE_MASK_BENCH_OUT") else {
        return;
    };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    let corpora = [
        ("safe-prose", "The reviewer checked the proposed change and its tests. Café 日本語 🦀 remain readable.\r\n".repeat(16384)),
        ("punctuation-prose", "Review notes: update widget_name (alpha-beta), tests=[passed]; URL https://example.invalid/docs?q=syntax. {\"max_tokens\": 4096}\r\n".repeat(16384)),
        ("dense-secrets", WITNESSES[1..].join("\r\n").repeat(2048)),
    ];
    for (name, text) in corpora {
        std::fs::write(out.join(format!("{name}.txt")), &text).unwrap();
        let expected = mask_text_regex(&text);
        assert_eq!(mask_text(&text), expected);
        println!(
            "input {name} bytes={} sha256={} masks={} output_sha256={}",
            text.len(),
            digest(text.as_bytes()),
            expected.1,
            digest(expected.0.as_bytes())
        );
        for sample in 0..11 {
            // Reverse order each pair to avoid systematically favouring warm caches.
            for optimized in if sample % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let started = Instant::now();
                let answer = if optimized {
                    mask_text(black_box(&text))
                } else {
                    mask_text_regex(black_box(&text))
                };
                black_box(&answer);
                let elapsed = started.elapsed();
                assert_eq!(answer, expected);
                println!(
                    "sample {name} variant={} sample={sample} ns={}",
                    if optimized { "candidate" } else { "reference" },
                    elapsed.as_nanos()
                );
            }
        }
    }
}

#[test]
#[ignore]
fn masking_transcript_integration_benchmark() {
    use crate::claude::{fixtures, transcript_page};
    use std::time::Instant;
    let Some(out) = std::env::var_os("HEADSTATE_MASK_BENCH_OUT") else {
        return;
    };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    for fixture in [fixtures::MESSAGES_10K, fixtures::TOOL_HEAVY_70MB] {
        let w = fixtures::write(fixture, &out).unwrap();
        println!(
            "fixture {} bytes={} sha256={}",
            fixture.name,
            w.bytes,
            digest(&std::fs::read(&w.path).unwrap())
        );
        for sample in 0..3 {
            let started = Instant::now();
            let index = transcript_page::build_index(
                &w.path,
                None,
                Instant::now() + transcript_page::INDEX_DEADLINE,
            )
            .unwrap();
            println!(
                "index {} sample={sample} ns={} records={}",
                fixture.name,
                started.elapsed().as_nanos(),
                index.record_count()
            );
        }
        for query in ["widget", "zzzabsentzzz", "hidden"] {
            for matching in [Matching::Unmasked, Matching::Masked] {
                for sample in 0..5 {
                    let started = Instant::now();
                    let answer =
                        transcript_page::find(&w.path, Some(query), None, matching).unwrap();
                    println!("find {} query={query} mode={matching:?} sample={sample} ns={} hits={} more={} complete={}",fixture.name,started.elapsed().as_nanos(),answer.hits.len(),answer.more,answer.complete);
                }
            }
        }
    }
}
