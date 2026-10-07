/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 设为 "1" 时使用 src/mock/ 的假数据 */
  readonly VITE_USE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
