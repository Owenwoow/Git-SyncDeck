export type SaveFieldState = "saving" | "saved" | "error";

export interface SettingsSnapshot<T extends object> {
  value: T | null;
  loading: boolean;
  saving: boolean;
  readError: string | null;
  error: string | null;
  fieldStates: Partial<Record<keyof T, SaveFieldState>>;
}

type Change<T extends object> = {
  id: number;
  patch: Partial<T>;
  action: () => Promise<T>;
  error?: string;
};

const message = (error: unknown) => error instanceof Error ? error.message : String(error);

/** 所有本地写入串行执行；显示值由已保存值和尚未完成的字段修改叠加得到。 */
export class SettingsQueue<T extends object> {
  private confirmed: T | null = null;
  private pending: Change<T>[] = [];
  private failed: Change<T>[] = [];
  private fieldStates: Partial<Record<keyof T, SaveFieldState>> = {};
  private latest = new Map<keyof T, number>();
  private serial: Promise<void> = Promise.resolve();
  private eventVersion = 0;
  private nextId = 0;
  private loading = true;
  private readError: string | null = null;
  private listeners = new Set<() => void>();
  private snapshot: SettingsSnapshot<T> = {
    value: null, loading: true, saving: false, readError: null, error: null, fieldStates: {},
  };
  private readonly read: () => Promise<T>;
  private readonly write: (patch: Partial<T>) => Promise<T>;

  constructor(read: () => Promise<T>, write: (patch: Partial<T>) => Promise<T>) {
    this.read = read;
    this.write = write;
  }

  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private keys(patch: Partial<T>) { return Object.keys(patch) as (keyof T)[]; }
  private current(change: Change<T>) {
    return this.keys(change.patch).every((key) => this.latest.get(key) === change.id);
  }

  private publish() {
    this.failed = this.failed.filter((change) => this.current(change));
    let value = this.confirmed ? { ...this.confirmed } : null;
    const fields = { ...this.fieldStates };
    for (const change of this.pending) {
      if (value) value = { ...value, ...change.patch };
      for (const key of this.keys(change.patch)) fields[key] = "saving";
    }
    this.snapshot = {
      value, loading: this.loading, saving: this.pending.length > 0,
      readError: this.readError, error: this.failed[this.failed.length - 1]?.error ?? null, fieldStates: fields,
    };
    this.listeners.forEach((listener) => listener());
  }

  /** 托盘变更的已保存快照，仍需叠加本地未完成修改。 */
  receive = (value: T) => {
    this.eventVersion++;
    this.confirmed = value;
    this.publish();
  };

  /** 真正重新读取；不调用保存接口，也不覆盖读取期间收到的较新事件。 */
  reload = (): Promise<void> => {
    const work = this.serial.then(async () => {
      this.loading = true;
      this.publish();
      const version = this.eventVersion;
      try {
        const value = await this.read();
        if (version === this.eventVersion) this.confirmed = value;
        this.readError = null;
      } catch (error) {
        this.readError = message(error);
        throw error;
      } finally {
        this.loading = false;
        this.publish();
      }
    });
    this.serial = work.catch(() => {});
    return work;
  };

  save = (patch: Partial<T>): Promise<void> => {
    if (this.keys(patch).length === 0) return Promise.resolve();
    return this.change(patch, () => this.write(patch));
  };

  change = (patch: Partial<T>, action: () => Promise<T>): Promise<void> => {
    const change: Change<T> = { id: ++this.nextId, patch, action };
    for (const key of this.keys(patch)) this.latest.set(key, change.id);
    this.pending.push(change);
    this.publish();
    const work = this.serial.then(async () => {
      const version = this.eventVersion;
      try {
        const value = await action();
        if (version === this.eventVersion) this.confirmed = value;
        else {
          // 写入期间托盘也改变了设置；重新读盘，避免旧响应盖掉托盘选择。
          const readVersion = this.eventVersion;
          try {
            const current = await this.read();
            if (readVersion === this.eventVersion) this.confirmed = current;
            this.readError = null;
          } catch (error) {
            this.readError = message(error);
          }
        }
        for (const key of this.keys(patch)) {
          if (this.latest.get(key) === change.id) this.fieldStates[key] = "saved";
        }
      } catch (error) {
        change.error = message(error);
        if (this.current(change)) this.failed.push(change);
        for (const key of this.keys(patch)) {
          if (this.latest.get(key) === change.id) this.fieldStates[key] = "error";
        }
        throw error;
      } finally {
        this.pending = this.pending.filter((entry) => entry.id !== change.id);
        this.publish();
      }
    });
    this.serial = work.catch(() => {});
    return work;
  };

  retry = async (): Promise<void> => {
    const changes = this.failed.filter((change) => this.current(change));
    this.failed = [];
    await Promise.all(changes.map((change) => this.change(change.patch, change.action)));
  };

  /** 用户明确放弃一组失败修改后，重试入口不能再悄悄应用它。 */
  discardFailed = (fields: (keyof T)[]): void => {
    const discarded = new Set(fields);
    this.failed = this.failed.flatMap((change) => {
      const remaining = Object.fromEntries(
        Object.entries(change.patch).filter(([key]) => !discarded.has(key as keyof T)),
      ) as Partial<T>;
      if (!this.keys(remaining).length) return [];
      if (this.keys(remaining).length === this.keys(change.patch).length) return [change];
      return [{ ...change, patch: remaining, action: () => this.write(remaining) }];
    });
    for (const field of fields) {
      if (this.fieldStates[field] === "error") delete this.fieldStates[field];
    }
    this.publish();
  };
}
