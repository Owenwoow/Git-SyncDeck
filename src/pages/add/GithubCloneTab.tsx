import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { AlertCircle, Check, CircleDashed, FolderOpen, Info, Loader2, ListFilter, Search, X } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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

/** 从 GitHub 克隆：列出仓库 → 勾选 → 克隆到本批保存目录并加入监控。 */
export function GithubCloneTab() {
  const { settings } = useSettings();
  const { reload, refresh, setCloning } = useProjects();
  const clone = useCloneState();
  const [input, setInput] = useState("");
  const [codeDir, setCodeDir] = useState<string | null>(null);
  const [fallbackDir, setFallbackDir] = useState("");
  const [listing, setListing] = useState<GithubListing | null>(null);
  const [loading, setLoading] = useState(false);
  const [picking, setPicking] = useState(false);
  const [checking, setChecking] = useState(false);
  const [targetsReady, setTargetsReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 勾选的仓库（克隆地址） */
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [onlyCloneable, setOnlyCloneable] = useState(false);
  const [hideForks, setHideForks] = useState(false);
  const [hideArchived, setHideArchived] = useState(false);
  const alive = useRef(true);
  const requestId = useRef(0);
  const listingRef = useRef(listing);
  listingRef.current = listing;
  const completedBatch = useRef(clone.running ? 0 : clone.batchId);
  const defaultDir = settings?.defaultCodeDir ?? fallbackDir;
  const dir = clone.running ? clone.parentDir ?? defaultDir : codeDir ?? defaultDir;
  const busy = loading || picking || checking || clone.running;
  const filterCount = Number(onlyCloneable) + Number(hideForks) + Number(hideArchived);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; requestId.current++; };
  }, []);

  // 第一次打开：输入框优先填上次的地址，没有就填 gh 登录的账号
  useEffect(() => {
    let alive = true;
    api
      .getCloneDefaults()
      .then((d) => {
        if (!alive) return;
        setFallbackDir(d.defaultCodeDir);
        setInput((cur) => cur || d.lastInput || d.ghLogin || "");
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const list = useCallback(async (text: string, parent: string) => {
    const id = ++requestId.current;
    setLoading(true);
    setError(null);
    setTargetsReady(false);
    try {
      const result = await api.listGithubRepos(text, parent);
      if (!alive.current || id !== requestId.current) return;
      setListing(result);
      setChecked(new Set());
      setTargetsReady(true);
    } catch (e) {
      if (!alive.current || id !== requestId.current) return;
      setError(errorMessage(e));
      setListing(null);
      setChecked(new Set());
    } finally {
      if (alive.current && id === requestId.current) setLoading(false);
    }
  }, []);

  const inspectDirectory = useCallback(async (target: string, restoreDefault = false) => {
    const id = ++requestId.current;
    const current = listingRef.current;
    setChecking(true);
    try {
      const entries = await api.inspectGithubTargets(current?.repos.map((e) => e.repo) ?? [], target);
      if (!alive.current || id !== requestId.current) return;
      setCodeDir(restoreDefault ? null : target);
      if (current) setListing({ ...current, repos: entries });
      const selectable = new Set(entries.filter((e) => !unselectableReason(e)).map((e) => e.repo.cloneUrl));
      setChecked((previous) => new Set([...previous].filter((u) => selectable.has(u))));
      setTargetsReady(true);
      setError(null);
      return selectable;
    } catch (e) {
      if (alive.current && id === requestId.current) {
        if (restoreDefault) setError(`默认目录检查失败：${errorMessage(e)}，请重新选择保存目录`);
        else toast.error("无法更改保存目录", { description: errorMessage(e) });
      }
    } finally {
      if (alive.current && id === requestId.current) setChecking(false);
    }
  }, []);

  // 用当前组件观察结束状态；离开页面后，旧组件的完成回调不再修改页面状态。
  useEffect(() => {
    if (clone.running || !clone.batchId || completedBatch.current === clone.batchId) return;
    completedBatch.current = clone.batchId;
    setCodeDir(null);
    setTargetsReady(false);
    setChecked(new Set());
    if (defaultDir) void inspectDirectory(defaultDir, true);
  }, [clone.running, clone.batchId, defaultDir, inspectDirectory]);

  async function pick() {
    if (busy) return;
    setPicking(true);
    try {
      const selected = await api.pickDirectory(dir || undefined);
      if (!selected || !alive.current) return;
      const previous = checked;
      const selectable = await inspectDirectory(selected);
      if (selectable && alive.current) {
        const removed = [...previous].filter((u) => !selectable.has(u)).length;
        if (removed) toast.info(`已取消 ${removed} 个不可克隆仓库的选择`);
      }
    } catch (e) {
      if (alive.current) toast.error("选择目录失败", { description: errorMessage(e) });
    } finally {
      if (alive.current) setPicking(false);
    }
  }

  function clearFilters() {
    setOnlyCloneable(false);
    setHideForks(false);
    setHideArchived(false);
  }

  function submit(ev: FormEvent) {
    ev.preventDefault();
    if (!input.trim() || !dir || busy) return;
    setQuery("");
    void list(input.trim(), dir);
  }

  const entries = listing?.repos ?? [];
  const selectable = useMemo(() => entries.filter((e) => !unselectableReason(e)), [entries]);
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries.filter((e) => (!onlyCloneable || !unselectableReason(e))
      && (!hideForks || !e.repo.fork)
      && (!hideArchived || !e.repo.archived)
      && (!q || [e.repo.name, e.repo.description ?? ""].some((s) => s.toLowerCase().includes(q))));
  }, [entries, query, onlyCloneable, hideForks, hideArchived]);
  const visibleSelectable = visible.filter((e) => !unselectableReason(e));
  const allVisibleChecked = visibleSelectable.length > 0 && visibleSelectable.every((e) => checked.has(e.repo.cloneUrl));
  const chosen = selectable.filter((e) => checked.has(e.repo.cloneUrl));
  const localCount = entries.filter((e) => e.local).length;
  const visibleUrls = new Set(visible.map((e) => e.repo.cloneUrl));
  const hiddenChosen = chosen.filter((e) => !visibleUrls.has(e.repo.cloneUrl)).length;

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
    if (chosen.length === 0 || busy || !targetsReady || !dir) return;
    const items = chosen.map((e) => ({ url: e.repo.cloneUrl, name: e.repo.name }));
    setChecked(new Set());
    await startClone(items, {
      setCloning,
      onFinished: () => {
        // 主页出现新项目；列表里刚克隆的变成"已在本地"；新项目已经联网检查过一次，再统一刷新一下状态
        void reload().catch((e) => toast.error("读取项目列表失败", { description: errorMessage(e) }));
        refresh().catch(() => undefined);
      },
    }, dir);
  }

  const showProgress = clone.rows.length > 0;

  return (
    <div className="space-y-5">
      <form onSubmit={submit} className="flex items-center gap-2">
        <Input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="GitHub 账号主页、用户名或仓库地址"
          className="h-9 min-w-0 flex-1 font-mono"
          spellCheck={false}
          aria-label="GitHub 账号主页、用户名或仓库地址"
        />
        <Button type="submit" disabled={!input.trim() || !dir || busy}>
          {loading ? <Loader2 className="animate-spin" /> : <ListFilter />}
          列出仓库
        </Button>
      </form>

      <div className="flex min-w-0 items-center gap-2 text-sm">
        <FolderOpen className="size-4 shrink-0 text-muted-foreground" />
        <span className="shrink-0 text-muted-foreground">保存到</span>
        <span className="min-w-0 flex-1 truncate font-mono select-text" title={dir}>{dir || "正在读取…"}</span>
        <Button variant="ghost" size="sm" onClick={() => void pick()} disabled={busy || !dir}>
          {(picking || checking) && <Loader2 className="animate-spin" />}
          更改…
        </Button>
      </div>

      {error && (
        <div className="flex items-start gap-2 rounded-lg border border-status-diverged/40 bg-status-diverged/8 px-4 py-3 text-sm">
          <AlertCircle className="mt-0.5 size-4 shrink-0 text-status-diverged" />
          <span className="min-w-0 break-words select-text">{error}</span>
        </div>
      )}

      {showProgress && <ProgressPanel rows={clone.rows} running={clone.running} parentDir={clone.parentDir} />}

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
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <div className="relative min-w-40 flex-1 sm:max-w-72">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="搜索仓库名或描述"
                  aria-label="搜索仓库名或描述"
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
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant={filterCount ? "secondary" : "outline"} size="sm" className="h-8">
                    <ListFilter />筛选{filterCount > 0 && ` · ${filterCount}`}
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="w-44">
                  <DropdownMenuCheckboxItem checked={onlyCloneable} onCheckedChange={setOnlyCloneable} onSelect={(e) => e.preventDefault()}>
                    仅看可克隆
                  </DropdownMenuCheckboxItem>
                  <DropdownMenuCheckboxItem checked={hideForks} onCheckedChange={setHideForks} onSelect={(e) => e.preventDefault()}>
                    隐藏 fork
                  </DropdownMenuCheckboxItem>
                  <DropdownMenuCheckboxItem checked={hideArchived} onCheckedChange={setHideArchived} onSelect={(e) => e.preventDefault()}>
                    隐藏已归档
                  </DropdownMenuCheckboxItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={clearFilters} disabled={filterCount === 0}>清除筛选</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
              {(query.trim() || filterCount > 0) && (
                <span className="text-xs text-muted-foreground tabular-nums">
                  匹配 {visible.length} / {entries.length} 个
                </span>
              )}
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto"
                onClick={toggleAllVisible}
                disabled={visibleSelectable.length === 0 || busy || !targetsReady}
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
              没有符合当前搜索和筛选条件的仓库
              <Button variant="link" size="sm" onClick={() => { setQuery(""); clearFilters(); }}>
                清除搜索和筛选
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
                  disabled={busy || !targetsReady}
                />
              ))}
            </ul>
          )}
        </section>
      ) : (
        !error && (
          <div className="rounded-lg border border-dashed py-12 text-center text-sm text-muted-foreground">
            填入 GitHub 账号主页，列出名下的仓库，勾选后克隆到上方保存目录
          </div>
        )
      )}

      {chosen.length > 0 && (
        <div className="sticky bottom-0 flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-popover px-4 py-3 shadow-lg">
          <span className="min-w-0 text-sm">
            已选 <b className="mx-0.5 text-status-synced">{chosen.length} 个</b>仓库
            {hiddenChosen > 0 && <span className="ml-1 text-muted-foreground">（其中 {hiddenChosen} 个被搜索或筛选隐藏）</span>}
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => setChecked(new Set())} disabled={busy}>
              撤销选择
            </Button>
            <Button onClick={() => void doClone()} disabled={busy || !targetsReady}>
              {clone.running && <Loader2 className="animate-spin" />}
              克隆 {chosen.length} 个
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

function ProgressPanel({ rows, running, parentDir }: { rows: CloneRow[]; running: boolean; parentDir: string | null }) {
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
      {parentDir && <p className="truncate border-b px-4 py-2 font-mono text-xs text-muted-foreground select-text" title={parentDir}>保存到 {parentDir}</p>}
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
