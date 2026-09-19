import { describe, expect, it } from "vitest";
import { PLACEHOLDER, PRESETS, templateProblem } from "./terminalTemplate";

/// The settings field's pre-save check.
///
/// Rust is the authority -- `claude::launch::Template::parse` is what
/// runs, and it re-validates every launch. This is deliberately a
/// subset, so the tests state what it DOES catch and, just as
/// importantly, that it does not reject things Rust accepts.
describe("templateProblem", () => {
  it("accepts every preset the panel offers", () => {
    // A preset that the validator rejects would be a button that saves
    // nothing, which is the worst possible first impression of the
    // feature.
    for (const p of PRESETS) {
      expect(templateProblem(p.template), p.label).toBeNull();
    }
  });

  it("treats empty as no terminal rather than as an error", () => {
    // Empty is the DEFAULT. Reporting it as a problem would put a red
    // message under a field nobody has touched.
    for (const raw of ["", "   ", "\t"]) {
      expect(templateProblem(raw)).toBeNull();
    }
  });

  it("requires the placeholder", () => {
    // Without it the terminal opens on nothing, which looks like it
    // worked -- the user believes Claude is starting.
    const p = templateProblem("open -a Terminal");
    expect(p).toContain(PLACEHOLDER);
  });

  it("refuses a repeated placeholder", () => {
    expect(templateProblem(`t ${PLACEHOLDER} ${PLACEHOLDER}`)).toContain("once");
  });

  it("refuses the placeholder as the program name", () => {
    // argv[0] is EXECUTED; a shell line there is run as a program name.
    expect(templateProblem(`${PLACEHOLDER} foo`)).toContain("program name");
  });

  it("reports an unclosed quote", () => {
    expect(templateProblem(`t -e "${PLACEHOLDER}`)).toContain("unclosed");
  });

  it("does not mistake an apostrophe inside double quotes for a quote", () => {
    // A path like `/Users/sam/it's here` is legitimate, and flagging it
    // would refuse a template that works.
    expect(templateProblem(`"/Users/sam/it's here/term" -e ${PLACEHOLDER}`)).toBeNull();
  });

  it("does not mistake an escaped quote for an opening one", () => {
    expect(templateProblem(`term -e \\" ${PLACEHOLDER}`)).toBeNull();
  });
});
