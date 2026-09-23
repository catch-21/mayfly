/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `"true"` for the testnet flavour (local pkarr relay on localhost:15411). */
  readonly VITE_TESTNET?: string;
  readonly VITE_BASE?: string;
}
