// 假"后端"的内存状态。只允许 src/api/ 引用；刷新页面后恢复初始数据。
import type { Project } from "@/types";
import { initialProjects } from "./projects";
import { unmonitoredRepos } from "./repos";

/** "磁盘上"存在的全部仓库（含未监控的），key 为项目 id */
const repos = new Map<string, Project>(
  [...initialProjects, ...unmonitoredRepos].map((p) => [p.id, structuredClone(p)]),
);

/** 监控中的项目 id，保持加入顺序 */
const monitored: string[] = initialProjects.map((p) => p.id);

let pickCount = 0;

export const db = {
  allRepos(): Project[] {
    return [...repos.values()];
  },
  getRepo(id: string): Project | undefined {
    return repos.get(id);
  },
  findRepoByPath(path: string): Project | undefined {
    const key = path.toLowerCase();
    return [...repos.values()].find((p) => p.path.toLowerCase() === key);
  },
  updateRepo(id: string, patch: Partial<Project>): Project {
    const current = repos.get(id);
    if (!current) throw new Error(`找不到项目 ${id}`);
    const next = { ...current, ...patch };
    repos.set(id, next);
    return next;
  },
  monitoredProjects(): Project[] {
    return monitored.map((id) => repos.get(id)!).filter(Boolean);
  },
  isMonitored(id: string): boolean {
    return monitored.includes(id);
  },
  monitor(id: string) {
    if (!monitored.includes(id)) monitored.push(id);
  },
  unmonitor(id: string) {
    const i = monitored.indexOf(id);
    if (i >= 0) monitored.splice(i, 1);
  },
  nextPickCount(): number {
    return pickCount++;
  },
};

/** 模拟耗时操作 */
export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** 在 [min, max] 之间随机取一个耗时，让进度看起来更自然 */
export function jitter(min: number, max: number): number {
  return Math.round(min + Math.random() * (max - min));
}

/** 返回深拷贝，避免界面直接改到"后端"数据 */
export function clone<T>(value: T): T {
  return structuredClone(value);
}
