// 「从 GitHub 克隆」的假数据：假账号 Owenwoow 名下的仓库。只允许 src/mock/api.ts 引用。
// 其中 blog、dotfiles、resume 的远程地址和 projects.ts 里已监控的项目相同 → 显示"已在本地"；
// notes-archive 在默认代码目录下已有同名文件夹（见 EXISTING_FOLDERS）；名字含 huge 的仓库克隆时模拟超时失败。
import type { GithubRepo } from "@/types";
import { minutesAgo } from "./projects";

export const MOCK_OWNER = "Owenwoow";

const DAY = 24 * 60;

function repo(
  name: string,
  daysAgo: number | null,
  description: string | null,
  flags: Partial<Pick<GithubRepo, "private" | "fork" | "archived">> = {},
): GithubRepo {
  return {
    name,
    fullName: `${MOCK_OWNER}/${name}`,
    cloneUrl: `https://github.com/${MOCK_OWNER}/${name}.git`,
    htmlUrl: `https://github.com/${MOCK_OWNER}/${name}`,
    private: false,
    fork: false,
    archived: false,
    pushedAt: daysAgo === null ? null : minutesAgo(Math.round(daysAgo * DAY)),
    description,
    ...flags,
  };
}

/** 已按"非 fork 非归档在前，各自按最近推送时间倒序"排好 */
export const mockGithubRepos: GithubRepo[] = [
  repo("Git-SyncDeck", 0.1, "在一个界面里查看多个本地 Git 项目与 GitHub 的同步状态"),
  repo("blog", 1, "Hexo 博客，靶机 Writeup 都在这里"),
  repo("kali-cheatsheet", 6, "渗透测试常用命令速查"),
  repo("client-site", 12, "接单项目的网站源码", { private: true }),
  repo("secret-notes", 20, "不公开的学习笔记", { private: true }),
  repo("resume", 45, "个人简历（Markdown + 导出脚本）"),
  repo("notes-archive", 80, "旧笔记归档"),
  repo("huge-dataset", 120, "很大的数据集（演示克隆超时）"),
  repo("dotfiles", 200, "Windows Terminal、PowerShell 和 Git 的配置"),
  repo("awesome-security", 9, "fork 来的安全资源清单", { fork: true }),
  repo("tutorial-fork", 150, null, { fork: true }),
  repo("legacy-php", 700, "早期的 PHP 练习，已归档", { archived: true }),
  repo("old-bot", 900, "停止维护的聊天机器人", { archived: true, private: true }),
];

/** 默认代码目录下已经存在的文件夹（不一定是仓库），用来演示"文件夹已存在" */
export const EXISTING_FOLDERS = ["notes-archive"];
