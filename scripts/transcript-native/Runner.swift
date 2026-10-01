// Developer-only synthetic harness. Private PID SPI is feature-detected;
// no application shipping target includes this file.
import AppKit
import WebKit

@_silgen_name("transcript_process_usage")
func processUsage(_ pid: Int32, _ resident: UnsafeMutablePointer<UInt64>, _ footprint: UnsafeMutablePointer<UInt64>) -> Int32

final class Runner: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    var window: NSWindow!
    var web: WKWebView!
    var timer: Timer?
    var busy = false
    var finished = false
    var phase = "baseline"
    var samples: [[String: Any]] = []
    var renderer: Int32?
    var visibilityObservers: [NSObjectProtocol] = []
    let output = CommandLine.arguments[2]
    let input = URL(string: CommandLine.arguments[1])!
    let started = ProcessInfo.processInfo.systemUptime

    func finish(_ status: String, _ reason: String, _ code: Int32) {
        guard !finished else { return }
        finished = true
        timer?.invalidate()
        let result: [String: Any] = [
            "status": status, "reason": reason, "samples": samples,
            "nativeBudgetAcceptance": "unmeasured",
            "timingMetric": "DOM-ready then double-rAF paint-opportunity proxy, not presentation",
            "memoryMetric": "separate whole-process RSS and physical footprint, not JS heap or transcript allocations",
            "unmeasured": ["exact JS heap", "script/layout attribution", "compositor presentation", "physical phone", "older navigation", "native eviction"]
        ]
        do {
            let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: output), options: .atomic)
        } catch { fputs("could not write native measurement result\n", stderr) }
        window?.close()
        exit(code)
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        guard input.host == "127.0.0.1", input.scheme == "http" else {
            finish("unavailable", "Only the owned loopback synthetic server is permitted", 3); return
        }
        let frame = NSRect(x: 0, y: 0, width: 1280, height: 800)
        window = NSWindow(contentRect: frame, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        web = WKWebView(frame: frame, configuration: config)
        web.navigationDelegate = self
        window.contentView = web
        window.title = "Headstate synthetic transcript measurement"
        window.center(); window.makeKeyAndOrderFront(nil); window.orderFrontRegardless()
        NSApp.activate(ignoringOtherApps: true)
        for event in [NSWindow.didChangeOcclusionStateNotification, NSApplication.didResignActiveNotification] {
            visibilityObservers.append(NotificationCenter.default.addObserver(forName: event, object: nil, queue: .main) { [weak self] note in
                guard let self, !self.samples.isEmpty else { return }
                // Inspect this owned window only, never other application windows.
                if !self.window.occlusionState.contains(.visible) || !NSApp.isActive {
                    self.finish("unavailable", "Owned window lost visibility during a measured phase", 3)
                }
            })
        }
        web.load(URLRequest(url: input))
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in self?.poll() }
    }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        let url = action.request.url
        decisionHandler(url?.host == input.host && url?.port == input.port ? .allow : .cancel)
    }
    func poll() {
        guard !finished else { return }
        if ProcessInfo.processInfo.systemUptime - started > 20 {
            finish("unavailable", "Timed out waiting for the synthetic harness/phase; inspect own runner log", 3); return
        }
        guard !busy else { return }
        busy = true
        web.evaluateJavaScript("({ready:document.body?.dataset.harness==='ready',visible:document.visibilityState==='visible'&&!window.__nativeHidden,phase:window.__nativePhase??null,rows:document.querySelectorAll('[data-message-id]').length,reads:window.__harness?.reads.length??null,refused:window.__harness?.refused.length??null})") { value, error in
            self.busy = false
            guard let state = value as? [String: Any], state["ready"] as? Bool == true else { return }
            guard self.window.isVisible, self.window.occlusionState.contains(.visible), NSApp.isActive, state["visible"] as? Bool == true else {
                self.finish("unavailable", "Window hidden, occluded or inactive: unlock the Mac and keep this owned window foreground; no timing acceptance", 3); return
            }
            if let count = state["refused"] as? Int, count != 0 {
                self.finish("unavailable", "Synthetic harness refused a command; no complete measurement", 3); return
            }
            if self.phase != "baseline" {
                guard let done = state["phase"] as? [String: Any], done["name"] as? String == self.phase else { return }
                if done["error"] != nil { self.finish("unavailable", "Synthetic phase did not finish", 3); return }
            }
            guard let memory = self.memory() else { return }
            var sample = memory
            sample["phase"] = self.phase; sample["visible"] = true
            sample["elapsedSeconds"] = ProcessInfo.processInfo.systemUptime - self.started
            sample["rows"] = state["rows"]; sample["reads"] = state["reads"]
            if let done = state["phase"] as? [String: Any] { sample["paintOpportunityMs"] = done["milliseconds"] }
            self.samples.append(sample)
            switch self.phase {
            case "baseline": self.phase = "open"
            case "open": self.phase = "append"
            case "append": self.phase = "idle"
            default: self.finish("measured-proxies", "Visible synthetic phases only; native budgets remain unmeasured", 0); return
            }
            self.drive()
        }
    }
    func memory() -> [String: Any]? {
        let selector = NSSelectorFromString("_webProcessIdentifier")
        guard web.responds(to: selector), let number = web.value(forKey: "_webProcessIdentifier") as? NSNumber else {
            finish("unavailable", "This WebKit does not expose its own renderer PID to the development harness", 3); return nil
        }
        let pid = number.int32Value, host = ProcessInfo.processInfo.processIdentifier
        guard pid > 0, pid != host, renderer == nil || renderer == pid else {
            finish("unavailable", "Renderer identity missing or changed during measurement", 3); return nil
        }
        renderer = pid
        var hr: UInt64 = 0, hf: UInt64 = 0, wr: UInt64 = 0, wf: UInt64 = 0
        guard processUsage(host, &hr, &hf) == 0, processUsage(pid, &wr, &wf) == 0 else {
            finish("unavailable", "Own-process RSS/footprint unavailable; never substitute host memory for WebContent", 3); return nil
        }
        return ["hostPID": host, "webContentPID": pid, "hostResidentBytes": hr, "hostPhysicalFootprintBytes": hf, "webContentResidentBytes": wr, "webContentPhysicalFootprintBytes": wf]
    }
    func drive() {
        // Fixed synthetic operations, no transcript contents enter output.
        let script = """
        void (async()=>{
          const name='\(phase)', start=performance.now();
          if(!window.__nativeVisibilityInstalled){
            window.__nativeVisibilityInstalled=true;
            document.addEventListener('visibilitychange',()=>{if(document.hidden)window.__nativeHidden=true;});
          }
          const wait=async test=>{const until=performance.now()+4000;while(!test()){if(performance.now()>until)throw Error('phase timeout');await new Promise(r=>setTimeout(r,25));}};
          try {
            if(name==='open'){
              [...document.querySelectorAll('button')].find(b=>b.textContent==='Open').click();
              await wait(()=>document.querySelector('[data-message-id]'));
            }else if(name==='append'){
              document.querySelector('button[aria-label^="Jump to the latest message"]')?.click();
              await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
              window.__harness.grow(1);await window.__harness.nudge(window.__harness.size+1);
              await wait(()=>window.__harness.queued===0);
            }else await new Promise(r=>setTimeout(r,2000));
            await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
            window.__nativePhase={name,milliseconds:performance.now()-start};
          }catch(e){window.__nativePhase={name,error:true};}
        })();
        """
        web.evaluateJavaScript(script) { _, error in
            if error != nil { self.finish("unavailable", "Could not drive the synthetic phase", 3) }
        }
    }
}
let app = NSApplication.shared
let runner = Runner()
app.delegate = runner
app.setActivationPolicy(.regular)
app.run()
