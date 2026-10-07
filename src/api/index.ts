// ★ 界面获取数据的唯一入口。组件只能 import from "@/api"，不能直接引用 src/mock/。
//
// 两套实现，签名完全一致：
// - 真实数据：./tauri.ts，调用 Rust 端的 Tauri 命令（系统 git 命令行）
// - 假数据：  src/mock/api.ts，第一阶段的 Demo 逻辑
//
// 切换方式：环境变量 VITE_USE_MOCK=1 时用假数据（npm run dev:mock / npm run tauri:mock）；
// 在普通浏览器里打开（不在 Tauri 窗口内）时也自动用假数据，方便继续调界面。
import * as mock from "@/mock/api";
import * as real from "./tauri";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** 演示模式：侧边栏显示"演示数据"标记，打开终端等操作只弹提示 */
export const isDemoMode = import.meta.env.VITE_USE_MOCK === "1" || !inTauri;

// 类型标注保证假数据实现和真实实现的签名一致
const impl: typeof real = isDemoMode ? mock : real;

export const {
  checkGit,
  listProjects,
  refreshStatus,
  addProjects,
  removeProject,
  setAutoCommit,
  pickDirectory,
  scanDirectory,
  syncAll,
  commitAndPush,
  getDiagnosticText,
  openInTerminal,
  openInEditor,
  getSettings,
  saveSettings,
} = impl;
