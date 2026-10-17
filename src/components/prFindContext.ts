import { createContext, useContext } from "react";
// Search temporarily reveals loaded disclosures; their manual state is retained.
export const PrFindExpanded = createContext(false);
export const usePrFindExpanded = () => useContext(PrFindExpanded);
