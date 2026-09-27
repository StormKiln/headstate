import { createContext } from "react";

/// Whether a fold is already inside a sheet (#1481).
///
/// The phone's tool chip opens the whole call in a bottom sheet. A fold
/// in there opening a SECOND sheet over the first would make the output
/// two taps away and stack two modal layers on a phone screen, so inside
/// a sheet a compact `Fold` opens in place instead -- and starts open,
/// because the reader tapped the chip to see exactly this.
export const InSheet = createContext(false);
