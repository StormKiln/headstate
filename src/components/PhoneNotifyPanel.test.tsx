import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const set = vi.fn(() => Promise.resolve());
const state: { prefs: unknown } = { prefs: undefined };

vi.mock("@/api/phoneNotify", () => ({
  usePhoneNotifyPrefs: () => ({ prefs: state.prefs, set }),
}));

import { PhoneNotifyPanel } from "./PhoneNotifyPanel";

beforeEach(() => {
  set.mockClear();
  state.prefs = { enabled: true, new_pr: true, health_battery: true, health_cpu: true, sessions: true, session_snippet: false };
});
afterEach(cleanup);

/// #789: the phone's OWN notification settings.
///
/// Separate from the desktop's because the desktop's `get_notify_prefs`
/// is `Class::Local` -- the phone cannot read or write it -- and because
/// the two devices are in different places: CI failures on the laptop you
/// are working at and only new pull requests on the phone in your pocket
/// is a reasonable thing to want, and one shared setting could not
/// express it.
describe("the phone's notification settings", () => {
  it("offers a master switch and one toggle per category", () => {
    render(<PhoneNotifyPanel />);
    expect(screen.getByRole("checkbox", { name: /phone notifications/i })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: /a pull request appears/i })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: /battery problems/i })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: /cpu is busy/i })).toBeTruthy();
  });

  it("silences one category without touching the others", () => {
    render(<PhoneNotifyPanel />);
    fireEvent.click(screen.getByRole("checkbox", { name: /battery problems/i }));
    expect(set).toHaveBeenCalledWith({
      enabled: true,
      new_pr: true,
      health_battery: false,
      health_cpu: true,
      sessions: true,
      session_snippet: false,
    });
  });

  /// The master switch keeps the choices underneath it, so turning
  /// notifications back on restores what was picked rather than a reset.
  it("keeps the per-category choices when the master switch goes off", () => {
    state.prefs = { enabled: true, new_pr: false, health_battery: true, health_cpu: true, sessions: true, session_snippet: false };
    render(<PhoneNotifyPanel />);
    fireEvent.click(screen.getByRole("checkbox", { name: /phone notifications/i }));
    expect(set).toHaveBeenCalledWith({
      enabled: false,
      new_pr: false,
      health_battery: true,
      health_cpu: true,
      sessions: true,
      session_snippet: false,
    });
  });

  it("disables the categories when the master switch is off", () => {
    state.prefs = { enabled: false, new_pr: true, health_battery: true, health_cpu: true, sessions: true, session_snippet: false };
    render(<PhoneNotifyPanel />);
    for (const name of [/a pull request appears/i, /battery problems/i, /cpu is busy/i]) {
      expect(screen.getByRole("checkbox", { name })).toHaveProperty("disabled", true);
    }
  });

  /// **The copy requirement from #789.** The companion reaches the
  /// desktop through the remote surface, so health data here is the
  /// DESKTOP's health. A row saying only "battery problems" would read as
  /// a setting about this phone's battery -- a different thing entirely,
  /// and one the user can already see in their status bar.
  it("names the machine the health categories are about", () => {
    render(<PhoneNotifyPanel />);
    expect(
      screen.getByRole("checkbox", { name: /battery problems on the paired mac/i }),
    ).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: /paired mac.*cpu/i })).toBeTruthy();
    expect(screen.getByText(/about the mac you paired with, not this phone/i)).toBeTruthy();
  });

  /// **The delivery tradeoff, stated.** iOS decides when a background
  /// refresh window opens, so this is "you will find out within the
  /// hour", not "instantly" -- and a user who expects instant and gets
  /// hourly concludes the feature is broken rather than that it is
  /// working as designed.
  it("says delivery is best-effort rather than instant", () => {
    render(<PhoneNotifyPanel />);
    expect(screen.getByText(/within the hour rather than the moment/i)).toBeTruthy();
    expect(screen.getByText(/wakes the app in the background/i)).toBeTruthy();
  });

  /// The first notification is what asks for permission, not launch --
  /// mirroring `notification_allowed`'s ask-once-before-first-use
  /// discipline. The panel says so, because otherwise a user who has
  /// ticked everything and seen no prompt has no way to know why.
  it("says the first notification is what asks permission", () => {
    render(<PhoneNotifyPanel />);
    expect(screen.getByText(/first one asks permission/i)).toBeTruthy();
  });

  /// #1486: the global Sessions switch, and the snippet setting that is
  /// OFF by default -- the lock screen names the project and the state
  /// only unless the owner asks for more.
  it("offers a Sessions switch and an opening-prompt setting that defaults off", () => {
    state.prefs = undefined;
    render(<PhoneNotifyPanel />);
    expect(screen.getByRole("checkbox", { name: /^sessions:/i })).toHaveProperty("checked", true);
    expect(screen.getByRole("checkbox", { name: /opening prompt/i })).toHaveProperty(
      "checked",
      false,
    );
  });

  it("switches sessions off without touching the other categories", () => {
    render(<PhoneNotifyPanel />);
    fireEvent.click(screen.getByRole("checkbox", { name: /^sessions:/i }));
    expect(set).toHaveBeenCalledWith(expect.objectContaining({ sessions: false, new_pr: true }));
  });

  it("disables the snippet setting while sessions are off", () => {
    state.prefs = { ...(state.prefs as object), sessions: false };
    render(<PhoneNotifyPanel />);
    expect(screen.getByRole("checkbox", { name: /opening prompt/i })).toHaveProperty(
      "disabled",
      true,
    );
  });

  /// **The owner's decision on #1486**: instant delivery is #1492, and
  /// this ships best-effort -- which the setting that enables it says.
  it("says session notifications are best-effort where they are enabled", () => {
    render(<PhoneNotifyPanel />);
    expect(screen.getByText(/best-effort, delivery can be delayed by ios/i)).toBeTruthy();
  });

  /// One render passes before the query resolves. The Rust default is
  /// ON, so the boxes must render checked rather than flickering from
  /// off to on -- which reads as the app changing a setting by itself.
  it("renders checked before the preferences have loaded", () => {
    state.prefs = undefined;
    render(<PhoneNotifyPanel />);
    for (const name of [
      /phone notifications/i,
      /a pull request appears/i,
      /battery problems/i,
      /cpu is busy/i,
    ]) {
      expect(screen.getByRole("checkbox", { name })).toHaveProperty("checked", true);
    }
  });
});
