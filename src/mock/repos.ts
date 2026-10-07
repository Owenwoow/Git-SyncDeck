// 扫描目录时会额外发现、但还没有加入监控的假仓库。
// 加入监控后，它们会以这里的数据出现在主页。
import type { Project } from "@/types";
import { DEFAULT_COMMIT_TEMPLATE } from "@/lib/status";
import { minutesAgo } from "./projects";

const GH = "https://github.com/Owenwoow";
const DAY = 24 * 60;

export const unmonitoredRepos: Project[] = [
  // ---- D:\code 下 ----
  {
    id: "p-old-portfolio",
    name: "old-portfolio",
    path: "D:\\code\\old-portfolio",
    branch: "master",
    remoteUrl: `${GH}/old-portfolio.git`,
    upstream: "origin/master",
    status: "synced",
    ahead: 0,
    behind: 0,
    changes: [],
    lastSyncAt: minutesAgo(64 * DAY),
    issue: null,
    checkError: null,
    autoCommit: false,
  },
  {
    id: "p-rust-learning",
    name: "rust-learning",
    path: "D:\\code\\rust-learning",
    branch: "main",
    remoteUrl: `${GH}/rust-learning.git`,
    upstream: "origin/main",
    status: "ahead",
    ahead: 3,
    behind: 0,
    changes: [],
    lastSyncAt: minutesAgo(12 * DAY),
    issue: null,
    checkError: null,
    autoCommit: false,
  },
  {
    id: "p-temp-vite-demo",
    name: "temp-vite-demo",
    path: "D:\\code\\temp-vite-demo",
    branch: "main",
    remoteUrl: null,
    upstream: null,
    status: "no-remote",
    ahead: 0,
    behind: 0,
    changes: [],
    lastSyncAt: null,
    issue: null,
    checkError: null,
    autoCommit: false,
  },
  {
    id: "p-jquery-lazyload",
    name: "jquery-lazyload",
    path: "D:\\code\\archive\\jquery-lazyload",
    branch: "master",
    remoteUrl: `${GH}/jquery-lazyload.git`,
    upstream: "origin/master",
    status: "synced",
    ahead: 0,
    behind: 0,
    changes: [],
    lastSyncAt: minutesAgo(400 * DAY),
    issue: null,
    checkError: null,
    autoCommit: false,
  },
  // ---- E:\work 下（"选择目录"切换到这里时能看到）----
  {
    id: "p-company-wiki",
    name: "company-wiki",
    path: "E:\\work\\company-wiki",
    branch: "main",
    remoteUrl: `${GH}/company-wiki.git`,
    upstream: "origin/main",
    status: "behind",
    ahead: 0,
    behind: 2,
    changes: [],
    lastSyncAt: minutesAgo(4 * DAY),
    issue: null,
    checkError: null,
    autoCommit: false,
  },
  {
    id: "p-report-generator",
    name: "report-generator",
    path: "E:\\work\\report-generator",
    branch: "main",
    remoteUrl: `${GH}/report-generator.git`,
    upstream: "origin/main",
    status: "synced",
    ahead: 0,
    behind: 0,
    changes: [],
    lastSyncAt: minutesAgo(DAY + 90),
    issue: null,
    checkError: null,
    autoCommit: false,
  },
];

/** "选择目录"时依次返回的假目录 */
export const pickableDirs = ["E:\\work", "D:\\code"];

export const defaultSettings = {
  defaultCodeDir: "D:\\code",
  commitTemplate: DEFAULT_COMMIT_TEMPLATE,
};
