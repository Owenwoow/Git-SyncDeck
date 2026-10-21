import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { AlertCircle, Check, CircleDashed, Info, Loader2, ListFilter, Search, X } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { formatRelative } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useProjects } from "@/state/projects";
import { useSettings } from "@/state/settings";
import type { GithubListing, GithubRepoEntry } from "@/types";
import { clearCloneResults, rowText, startClone, useCloneState, type CloneRow } from "./cloneStore";

/** 一行能不能勾：已在本地、同名文件夹已存在的不能克隆 */
function unselectableReason(e: GithubRepoEntry): string | null {
  if (e.local) return `已在本地：${e.local}`;
  if (e.targetExists) return `文件夹已存在：${e.target}`;
  return null;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** 从 GitHub 克隆：填账号主页（或用户名、仓库地址）→ 列出仓库 → 勾选 → 克隆到默认代码目录并加入监控 */
export function GithubCloneTab() {
  const { settings } = useSettings();
  const { reload, refresh, setCloning } = useProjects();
  const clone = useCloneState();
  const [input, setInput] = useState("");
  const [codeDir, setCodeDir] = useState<string | null>(null);
  const [listing, setListing] = useState<GithubListing | null>(null);
  const [listedInput, setListedInput] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 勾选的仓库（克隆地址） */
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");

  // 第一次打开：输入框优先填上次的地址，没有就填 gh 登录的账号
  useEffect(() => {
    let alive = true;
    api
      .getCloneDefaults()
      .then((d) => {
        if (!alive) return;
        setCodeDir(d.defaultCodeDir);
        setInput((cur) => cur || d.lastInput || d.ghLogin || "");
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const list = useCallback(async (text: string, keepChecked: boolean) => {
    setLoading(true);
    setError(null);
    try {
      const result = await api.listGithubRepos(text);
      setListing(result);
      setListedInput(text);
      setChecked((cur) => {
        if (!keepChecked) return new Set();
        // 保留还能勾的
        const ok = new Set(result.repos.filter((e) => !unselectableReason(e)).map((e) => e.repo.cloneUrl));
        return new Set([...cur].filter((u) => ok.has(u)));
      });
    } catch (e) {
      setError(errorMessage(e));
      if (!keepChecked) setListing(null);
    } finally {
      setLoading(false);
    }
  }, []);

  function submit(ev: FormEvent) {
    ev.preventDefault();
    if (!input.trim() || loading || clone.running) return;
    setQuery("");
    void list(input.trim(), false);
  }

  const entries = listing?.repos ?? [];
  const selectable = useMemo(() => entries.filter((e) => !unselectableReason(e)), [entries]);
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((e) => [e.repo.name, e.repo.description ?? ""].some((s) => s.toLowerCase().includes(q)));
  }, [entries, query]);
  const visibleSelectable = visible.filter((e) => !unselectableReason(e));
  const allVisibleChecked = visibleSelectable.length > 0 && visibleSelectable.every((e) => checked.has(e.repo.cloneUrl));
  const chosen = selectable.filter((e) => checked.has(e.repo.cloneUrl));
  const localCount = entries.filter((e) => e.local).length;

  function toggle(url: string, on: boolean) {
    setChecked((s) => {
      const next = new Set(s);
      if (on) next.add(url);
      else next.delete(url);
      return next;
    });
  }

  function toggleAllVisible() {
    setChecked((s) => {
      const next = new Set(s);
      for (const e of visibleSelectable) {
        if (allVisibleChecked) next.delete(e.repo.cloneUrl);
        else next.add(e.repo.cloneUrl);
      }
      return next;
    });
  }

  async function doClone() {
    if (chosen.length === 0 || clone.running) return;
    const items = chosen.map((e) => ({ url: e.repo.cloneUrl, name: e.repo.name }));
    setChecked(new Set());
    await startClone(items, {
      setCloning,
      onFinished: () => {
        // 主页出现新项目；列表里刚克隆的变成"已在本地"；新项目已经联网检查过一次，再统一刷新一下状态
        void reload().catch((e) => toast.error("读取项目列表失败", { description: errorMessage(e) }));
        if (listedInput) void list(listedInput, true);
        refresh().catch(() => undefined);
      },
    });
  }

  const dir = codeDir ?? settings?.defaultCodeDir ?? "";
  const showProgress = clone.rows.length > 0;

  return (
    <div className="space-y-5">
      <form onSubmit={submit} className="flex items-center gap-2">
        <Input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="GitHub 账号主页、用户名或仓库地址"
          className="h-9 flex-1 font-mono"
          spellCheck={false}
          aria-label="GitHub 账号主页、用户名或仓库地址"
        />
        <Button type="submit" disabled={!input.trim() || loading || clone.running}>
          {loading ? <Loader2 className="animate-spin" /> : <ListFilter />}
          列出仓库
        </Button>
      </form>

      {error && (
        <div className="flex items-start gap-2 rounded-lg border border-status-diverged/40 bg-status-diverged/8 px-4 py-3 text-sm">
          <AlertCircle className="mt-0.5 size-4 shrink-0 text-status-diverged" />
          <span className="min-w-0 break-words select-text">{error}</span>
        </div>
      )}

      {showProgress && <ProgressPanel rows={clone.rows} running={clone.running} />}

      {loading && !listing ? (
        <div className="space-y-2">
          {[0, 1, 2, 3, 4].map((i) => (
            <Skeleton key={i} className="h-14 rounded-lg" />
          ))}
        </div>
      ) : listing ? (
        <section>
          {listing.notice && (
            <div className="mb-3 flex items-start gap-2 rounded-lg border border-status-dirty/40 bg-status-dirty/8 px-4 py-3 text-sm">
              <Info className="mt-0.5 size-4 shrink-0 text-status-dirty" />
              <span className="min-w-0 break-words">{listing.notice}</span>
            </div>
          )}
          <div className="mb-2 flex items-baseline justify-between gap-3">
            <h2 className="text-sm font-medium">
              <span className="font-mono">{listing.owner}</span> 名下有 {entries.length} 个仓库
              {listing.source === "public" ? "（只含公开仓库）" : ""}
            </h2>
            <span className="shrink-0 text-xs text-muted-foreground">已在本地 {localCount} 个</span>
          </div>

          {entries.length > 0 && (
            <div className="mb-2 flex items-center gap-3">
              <div className="relative w-72 max-w-full">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="搜索仓库名或描述"
                  className="h-8 pr-8 pl-8"
                />
                {query && (
                  <button
                    type="button"
                    onClick={() => setQuery("")}
                    className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground"
                    aria-label="清空搜索"
                  >
                    <X className="size-3.5" />
                  </button>
                )}
              </div>
              {query.trim() && (
                <span className="text-xs text-muted-foreground">
                  匹配 {visible.length} / {entries.length} 个
                </span>
              )}
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto"
                onClick={toggleAllVisible}
                disabled={visibleSelectable.length === 0 || clone.running}
              >
                {allVisibleChecked ? "取消全选" : "全选可克隆的"}
              </Button>
            </div>
          )}

          {entries.length === 0 ? (
            <div className="rounded-lg border border-dashed py-12 text-center text-sm text-muted-foreground">
              这个账号下没有仓库
            </div>
          ) : visible.length === 0 ? (
            <div className="rounded-lg border border-dashed py-12 text-center text-sm text-muted-foreground">
              没有匹配"{query.trim()}"的仓库
              <Button variant="link" size="sm" onClick={() => setQuery("")}>
                清除搜索
              </Button>
            </div>
          ) : (
            <ul className="divide-y overflow-hidden rounded-lg border bg-card">
              {visible.map((e) => (
                <RepoRow
                  key={e.repo.cloneUrl}
                  entry={e}
                  checked={checked.has(e.repo.cloneUrl)}
                  onCheckedChange={(on) => toggle(e.repo.cloneUrl, on)}
                  disabled={clone.running}
                />
              ))}
            </ul>
          )}
        </section>
      ) : (
        !error && (
          <div className="rounded-lg border border-dashed py-12 text-center text-sm text-muted-foreground">
            填入 GitHub 账号主页，列出名下的仓库，勾选后克隆到默认代码目录
          </div>
        )
      )}

      {chosen.length > 0 && (
        <div className="sticky bottom-0 flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-popover px-4 py-3 shadow-lg">
          <span className="min-w-0 text-sm">
            将克隆 <b className="mx-0.5 text-status-synced">{chosen.length} 个</b>仓库到
            <span className="ml-1 font-mono break-all">{dir}</span>，并加入监控
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => setChecked(new Set())} disabled={clone.running}>
              撤销选择
            </Button>
            <Button onClick={() => void doClone()} disabled={clone.running}>
              {clone.running && <Loader2 className="animate-spin" />}
              克隆 {chosen.length} 个到 {dir}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

function ProgressPanel({ rows, running }: { rows: CloneRow[]; running: boolean }) {
  const finished = rows.filter((r) => r.status === "done" || r.status === "failed").length;
  return (
    <section className="rounded-lg border bg-card">
      <div className="flex items-center justify-between gap-3 border-b px-4 py-2.5">
        <h2 className="text-sm font-medium">
          {running ? `正在克隆（${finished} / ${rows.length}）` : `克隆结束（${finished} / ${rows.length}）`}
        </h2>
        {!running && (
          <Button variant="ghost" size="sm" onClick={clearCloneResults}>
            关闭
          </Button>
        )}
      </div>
      <ul className="divide-y">
        {rows.map((r) => (
          <li key={r.url} className="px-4 py-2.5">
            <div className="flex items-center gap-3">
              <span className="min-w-0 flex-1 truncate text-sm font-medium">{r.name}</span>
              <span
                className={cn(
                  "inline-flex shrink-0 items-center gap-1.5 text-xs",
                  r.status === "done" && "text-status-synced",
                  r.status === "failed" && "text-status-diverged",
                  r.status === "cloning" && "text-status-syncing",
                  r.status === "waiting" && "text-muted-foreground",
                )}
              >
                {r.status === "waiting" && <CircleDashed className="size-3.5" />}
                {r.status === "cloning" && <Loader2 className="size-3.5 animate-spin" />}
                {r.status === "done" && <Check className="size-3.5" />}
                {r.status === "failed" && <AlertCircle className="size-3.5" />}
                {rowText(r)}
              </span>
            </div>
            {r.status === "cloning" && (
              <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-muted">
                <div
                  className="h-full rounded-full bg-status-syncing transition-[width] duration-200"
                  style={{ width: `${r.percent ?? 0}%` }}
                />
              </div>
            )}
            {r.status === "failed" && r.error && (
              <p className="mt-1 text-xs break-words whitespace-pre-line text-status-diverged select-text">{r.error}</p>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Tag({ children, tone }: { children: string; tone?: "private" }) {
  return (
    <span
      className={cn(
        "shrink-0 rounded px-1.5 py-px text-[11px]",
        tone === "private" ? "bg-status-dirty/12 text-status-dirty" : "bg-muted text-muted-foreground",
      )}
    >
      {children}
    </span>
  );
}

function RepoRow({
  entry: e,
  checked,
  onCheckedChange,
  disabled,
}: {
  entry: GithubRepoEntry;
  checked: boolean;
  onCheckedChange: (on: boolean) => void;
  disabled: boolean;
}) {
  const reason = unselectableReason(e);
  const r = e.repo;
  return (
    <li>
      <label
        className={cn(
          "flex items-center gap-3 px-4 py-2.5 transition-colors",
          reason ? "cursor-default" : "cursor-pointer hover:bg-accent/50",
          checked && "bg-status-synced/6",
        )}
      >
        <Checkbox
          checked={checked}
          onCheckedChange={(v) => onCheckedChange(v === true)}
          disabled={disabled || reason !== null}
        />
        <div className={cn("min-w-0 flex-1", reason && "opacity-60")}>
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-sm font-medium">{r.name}</span>
            {r.private && <Tag tone="private">私有</Tag>}
            {r.fork && <Tag>fork</Tag>}
            {r.archived && <Tag>已归档</Tag>}
          </div>
          {r.description && <div className="mt-0.5 truncate text-xs text-muted-foreground">{r.description}</div>}
          {reason && <div className="mt-0.5 truncate font-mono text-xs text-muted-foreground">{reason}</div>}
        </div>
        {reason && (
          <span className="shrink-0 rounded bg-primary/8 px-1.5 py-px text-[11px] text-foreground/70">
            {e.local ? "已在本地" : "文件夹已存在"}
          </span>
        )}
        <span className="hidden w-20 shrink-0 text-right text-xs text-muted-foreground @2xl:block">
          {r.pushedAt ? formatRelative(r.pushedAt) : ""}
        </span>
      </label>
    </li>
  );
}
