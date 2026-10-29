import { create } from "zustand";

/// Whether the Go to ticket dialog is open (#2040). Its own store, so the
/// app-wide shortcut, the header button and the dialog agree without the
/// shell holding the state.
interface GoToTicketState {
  open: boolean;
  setOpen: (open: boolean) => void;
}

export const useGoToTicket = create<GoToTicketState>()((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));
