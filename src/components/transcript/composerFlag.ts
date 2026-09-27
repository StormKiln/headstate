/// 7.10 flips this (#1491). Off, the viewer's composer slot is laid out
/// but hidden and holds nothing, and no send path exists anywhere.
///
/// Its own module so a test can turn it on (`vi.mock`) without a
/// build-time switch that production could also reach.
export const COMPOSER_ENABLED = false;
