import { call } from "./transport";

// Manual local operations: no provider queries, polling or cache retirement.
// Native enrollment verifies the existing desktop identity before persistence.
export const getConnectionAddresses = () => call<string[]>("get_connection_addresses");
export const addConnectionAddress = (address: string) =>
  call<void>("add_connection_address", { address });
export const checkDesktopConnection = () => call<void>("check_desktop_connection");
export const getRemoteConnectionAddresses = () => call<string[]>("get_remote_connection_addresses");
