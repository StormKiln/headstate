import { vi } from "vitest";
const openUrl = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: (u: string) => openUrl(u) }));
const mobile = vi.hoisted(() => ({ value: false }));
vi.mock("../lib/useIsMobile", () => ({ useIsMobile: () => mobile.value }));
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { loadedGrammars, runJob } from "../lib/highlightCore";
import type { GrammarName } from "../lib/highlightLangs";
import { MAX_CHARS, MAX_LINES } from "../lib/highlightLangs";
import { TranscriptMarkdown } from "./TranscriptMarkdown";

/// A controllable `IntersectionObserver`: nothing is visible until the
/// test says so. jsdom has none of its own.
class FakeObserver {
  static all: FakeObserver[] = [];
  observed: Element[] = [];
  constructor(private readonly cb: IntersectionObserverCallback) {
    FakeObserver.all.push(this);
  }
  observe(el: Element) {
    this.observed.push(el);
  }
  disconnect() {
    this.observed = [];
  }
  unobserve() {}
  takeRecords() {
    return [];
  }
  /// Brings every element this observer watches into view.
  reveal() {
    const entries = this.observed.map(
      (target) => ({ target, isIntersecting: true }) as IntersectionObserverEntry,
    );
    if (entries.length) this.cb(entries, this as unknown as IntersectionObserver);
  }
}

/// The highlighting worker, run in-process. jsdom has no `Worker`; this
/// runs the same `runJob` the real worker does, asynchronously, and
/// answers in the real worker's message shape.
class InlineWorker extends EventTarget {
  postMessage(job: { id: number; code: string; grammar: GrammarName }) {
    void runJob(job.code, job.grammar).then(
      (tree) => this.dispatchEvent(new MessageEvent("message", { data: { id: job.id, tree } })),
      () => this.dispatchEvent(new MessageEvent("message", { data: { id: job.id, failed: true } })),
    );
  }
  terminate() {}
}

beforeEach(() => {
  vi.stubGlobal("Worker", InlineWorker);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  FakeObserver.all = [];
  mobile.value = false;
});

const fence = (lang: string, body: string) => "```" + lang + "\n" + body + "\n```";

describe("TranscriptMarkdown: nothing is fetched", () => {
  // The load-bearing property. Transcript text is attacker-influenced
  // (tool output, fetched pages), and a remote image in it would report
  // the reader's IP to its host the moment the transcript rendered.
  it("never creates an element that could request an image", () => {
    const created: string[] = [];
    const real = document.createElement.bind(document);
    vi.spyOn(document, "createElement").mockImplementation(
      (tag: string, opts?: ElementCreationOptions) => {
        created.push(tag.toLowerCase());
        return real(tag, opts);
      },
    );
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    const { container } = render(
      <TranscriptMarkdown>
        {[
          "![tracker](https://tracker.example/pixel.png)",
          "",
          "![ref][r]",
          "",
          "[r]: https://tracker.example/ref.png",
          "",
          '<img src="https://tracker.example/raw.png">',
          "",
          '<picture><source srcset="https://tracker.example/s.png"></picture>',
          "",
          '<video poster="https://tracker.example/v.png"></video>',
        ].join("\n")}
      </TranscriptMarkdown>,
    );
    for (const tag of ["img", "picture", "source", "video", "audio", "iframe", "object", "embed"]) {
      expect(created, `created a <${tag}>`).not.toContain(tag);
    }
    expect(container.querySelectorAll("[src], [srcset], [poster]")).toHaveLength(0);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("shows the address of an image it did not load", () => {
    render(<TranscriptMarkdown>{"![a chart](https://tracker.example/pixel.png)"}</TranscriptMarkdown>);
    const ph = screen.getByRole("img", { name: "Image not loaded: a chart" });
    expect(ph.textContent).toContain("https://tracker.example/pixel.png");
    expect(ph.tagName).toBe("SPAN");
  });
});

describe("TranscriptMarkdown: raw HTML is text", () => {
  it("prints HTML as its source instead of rendering it", () => {
    const { container } = render(
      <TranscriptMarkdown>
        {"before <b>bold</b> after\n\n<script>window.evil=1</script>\n\n<details open><summary>s</summary>x</details>"}
      </TranscriptMarkdown>,
    );
    expect(container.querySelector("b")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("details")).toBeNull();
    expect(container.textContent).toContain("before <b>bold</b> after");
    expect(container.textContent).toContain("<script>window.evil=1</script>");
    expect(container.textContent).toContain("<details open>");
  });

  it("still renders markdown itself", () => {
    const { container } = render(
      <TranscriptMarkdown>{"# Title\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |"}</TranscriptMarkdown>,
    );
    expect(container.querySelector("h1")?.textContent).toBe("Title");
    expect(container.querySelectorAll("li")).toHaveLength(2);
    expect(container.querySelector("table")).toBeTruthy();
  });
});

describe("TranscriptMarkdown: links", () => {
  it("opens http(s) links in the system browser and shows the host", () => {
    const { container } = render(
      <TranscriptMarkdown>{"[the docs](https://docs.example.org/a/b)"}</TranscriptMarkdown>,
    );
    const a = container.querySelector("a");
    expect(a?.getAttribute("href")).toBe("https://docs.example.org/a/b");
    expect(a?.textContent).toContain("the docs");
    expect(a?.textContent).toContain("(docs.example.org)");
    fireEvent.click(a as Element);
    expect(openUrl).toHaveBeenCalledWith("https://docs.example.org/a/b");
  });

  it("does not make a link of anything that is not http(s)", () => {
    const { container } = render(
      <TranscriptMarkdown>
        {"[js](javascript:alert(1)) [file](src/main.ts) [f](file:///etc/hosts)"}
      </TranscriptMarkdown>,
    );
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toContain("js");
    expect(container.textContent).toContain("file");
  });
});

describe("TranscriptMarkdown: code blocks", () => {
  it("labels the language and copies the source", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    render(<TranscriptMarkdown>{fence("rust", 'fn main() {\n    println!("hi");\n}')}</TranscriptMarkdown>);
    expect(screen.getByText("rust")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('fn main() {\n    println!("hi");\n}'));
    await screen.findByRole("button", { name: "Copied" });
  });

  it("wraps lines on the phone and scrolls them on the desktop", () => {
    const { container, rerender } = render(<TranscriptMarkdown>{fence("", "x")}</TranscriptMarkdown>);
    expect(container.querySelector("pre")?.className).toContain("overflow-x-auto");
    mobile.value = true;
    rerender(<TranscriptMarkdown>{fence("", "y")}</TranscriptMarkdown>);
    expect(container.querySelector("pre")?.className).toContain("whitespace-pre-wrap");
  });

  it("keeps inline code a chip, not a block", () => {
    const { container } = render(<TranscriptMarkdown>{"run `make lint` first"}</TranscriptMarkdown>);
    expect(container.querySelector("pre")).toBeNull();
    expect(container.querySelector("code")?.textContent).toBe("make lint");
  });
});

describe("TranscriptMarkdown: highlighting is lazy and bounded", () => {
  beforeEach(() => {
    vi.stubGlobal("IntersectionObserver", FakeObserver);
  });

  it("highlights a block only once it is seen, loading only its grammar", async () => {
    const { container } = render(
      <TranscriptMarkdown>{fence("python", "def f(x):\n    return x + 1  # one")}</TranscriptMarkdown>,
    );
    // Give any eager work every chance to run.
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(container.querySelector(".token")).toBeNull();
    expect(loadedGrammars()).not.toContain("python");

    act(() => FakeObserver.all.forEach((o) => o.reveal()));
    await waitFor(() => expect(container.querySelector("[data-highlighted]")).toBeTruthy());
    expect(loadedGrammars()).toContain("python");
    // A grammar nobody asked for is never fetched.
    expect(loadedGrammars()).not.toContain("swift");
    const keyword = [...container.querySelectorAll(".token")].find((e) => e.textContent === "def");
    expect(keyword?.className).toContain("text-[#ff7b72]");
    // The text is unchanged by the colouring.
    expect(container.querySelector("pre")?.textContent).toBe("def f(x):\n    return x + 1  # one");
  });

  it("does not highlight a block over the line cap, even once seen", async () => {
    const big = Array.from({ length: MAX_LINES + 1 }, (_, i) => `let v${i} = ${i};`).join("\n");
    const { container } = render(<TranscriptMarkdown>{fence("typescript", big)}</TranscriptMarkdown>);
    act(() => FakeObserver.all.forEach((o) => o.reveal()));
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(container.querySelector(".token")).toBeNull();
    expect(container.textContent).toContain(`${(MAX_LINES + 1).toLocaleString()} lines, shown without highlighting`);
    expect(container.querySelector("pre")?.textContent).toBe(big);
  });

  it("does not highlight a block over the character cap, even on one line", async () => {
    const big = "x".repeat(MAX_CHARS + 1);
    const { container } = render(<TranscriptMarkdown>{fence("js", big)}</TranscriptMarkdown>);
    act(() => FakeObserver.all.forEach((o) => o.reveal()));
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(container.querySelector(".token")).toBeNull();
    expect(container.textContent).toContain("shown without highlighting");
  });

  it("leaves an unknown language plain and labelled", async () => {
    const { container } = render(<TranscriptMarkdown>{fence("brainfuck", "+++[>+<-]")}</TranscriptMarkdown>);
    act(() => FakeObserver.all.forEach((o) => o.reveal()));
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(container.querySelector(".token")).toBeNull();
    expect(screen.getByText("brainfuck")).toBeTruthy();
    expect(container.querySelector("pre")?.textContent).toBe("+++[>+<-]");
  });
});

describe("TranscriptMarkdown: a block that grows", () => {
  // A live transcript extends its last block. The tree for the old text
  // must never be shown against the new text.
  it("never shows a tree computed for different text", async () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    const { container, rerender } = render(<TranscriptMarkdown>{fence("json", "[1]")}</TranscriptMarkdown>);
    await waitFor(() => expect(container.querySelector("[data-highlighted]")).toBeTruthy());
    const pre = container.querySelector("pre");
    rerender(<TranscriptMarkdown>{fence("json", "[1, 2]")}</TranscriptMarkdown>);
    // The SAME block, updated -- not torn down and rebuilt, which would
    // throw its highlighting away on every line a live session prints.
    expect(container.querySelector("pre")).toBe(pre);
    expect(container.querySelector("pre")?.textContent).toBe("[1, 2]");
    await waitFor(() => expect(container.querySelector("[data-highlighted]")).toBeTruthy());
    expect(container.querySelector("pre")?.textContent).toBe("[1, 2]");
  });
});

describe("TranscriptMarkdown without IntersectionObserver", () => {
  // jsdom's own condition. "Never seen" would leave every block plain
  // forever; a mounted block is highlighted instead.
  it("highlights mounted blocks", async () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    const { container } = render(<TranscriptMarkdown>{fence("json", '{"a": 1}')}</TranscriptMarkdown>);
    await waitFor(() => expect(container.querySelector("[data-highlighted]")).toBeTruthy());
    expect(container.querySelector(".token")).toBeTruthy();
  });
});
