import { QueryClient } from "@tanstack/react-query";

/// The app's query client, with retries OFF by default (#1955).
///
/// TanStack's own default is three retries with exponential backoff, so a
/// bare `new QueryClient()` turned every failure into four attempts and
/// about seven seconds of loading before the error showed: a refused
/// discovery walk ran four times, and a failing source refresh multiplied
/// backend calls that already carry their own attempt limit. #846 decided
/// that filesystem walks must not retry, and opting each hook out one at
/// a time left the ones nobody remembered. Off by default inverts that:
/// a query that genuinely benefits from a second attempt says so where it
/// is declared (the desktop auth check in `AuthGate` does).
///
/// A function rather than a shared instance so tests build the SAME
/// configuration production runs, not a hand-written lookalike that
/// happens to agree with it today.
export function createQueryClient(): QueryClient {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}
