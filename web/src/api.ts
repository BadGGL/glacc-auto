// 浏览器端 API 封装：同源 fetch + Cookie 会话。
// 所有写请求带 X-GLACC-CSRF: 1；后端同时校验 Origin/Referer 与 SameSite=Strict Cookie。

import type {
  Account,
  AppSettings,
  ClaimProgress,
  HistoryDay,
  HomeSnapshot,
  LoginResult,
  ScheduleRule,
  SmsCodeResult,
  TransferResult,
} from "./types";

const CSRF_HEADER = "X-GLACC-CSRF";

class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (init.body !== undefined && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  headers.set(CSRF_HEADER, "1");
  const res = await fetch(path, {
    ...init,
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    let message = res.statusText;
    try {
      const body = (await res.json()) as { error?: string };
      if (body.error) message = body.error;
    } catch {
      // 非 JSON 错误体，保留状态文本
    }
    throw new ApiError(res.status, message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

function jsonBody(value: unknown): string {
  return JSON.stringify(value);
}

export interface AuthState {
  authenticated: boolean;
  passwordSet: boolean;
}

export interface LogPage {
  fileName: string;
  exists: boolean;
  lines: string[];
  truncated: boolean;
}

export const api = {
  authState(): Promise<AuthState> {
    return request<AuthState>("/api/auth/state");
  },
  login(password: string): Promise<{ ok: boolean }> {
    return request("/api/auth/login", { method: "POST", body: jsonBody({ password }) });
  },
  logout(): Promise<{ ok: boolean }> {
    return request("/api/auth/logout", { method: "POST", body: "{}" });
  },

  getAccounts(): Promise<Account[]> {
    return request<Account[]>("/api/accounts");
  },
  getSnapshot(): Promise<HomeSnapshot> {
    return request<HomeSnapshot>("/api/snapshot");
  },
  getClaimProgress(): Promise<ClaimProgress> {
    return request<ClaimProgress>("/api/claim/progress");
  },
  startClaim(scope = "all"): Promise<{ started: boolean }> {
    return request("/api/claim/start", { method: "POST", body: jsonBody({ scope }) });
  },
  cancelClaim(): Promise<{ cancelling: boolean }> {
    return request("/api/claim/cancel", { method: "POST", body: "{}" });
  },

  getSettings(): Promise<AppSettings> {
    return request<AppSettings>("/api/settings").then((s) => ({
      ...s,
      scheduledEnabled: false,
      scheduledTime: s.scheduledTime || "08:00",
    }));
  },
  saveSettings(settings: AppSettings): Promise<AppSettings> {
    return request<AppSettings>("/api/settings", {
      method: "POST",
      body: jsonBody(settings),
    });
  },

  getSchedules(): Promise<ScheduleRule[]> {
    return request<ScheduleRule[]>("/api/schedules");
  },
  createSchedule(rule: Partial<ScheduleRule>): Promise<ScheduleRule> {
    return request("/api/schedules", { method: "POST", body: jsonBody(rule) });
  },
  updateSchedule(id: string, rule: Partial<ScheduleRule>): Promise<ScheduleRule> {
    return request(`/api/schedules/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: jsonBody(rule),
    });
  },
  deleteSchedule(id: string): Promise<{ deleted: boolean }> {
    return request(`/api/schedules/${encodeURIComponent(id)}`, {
      method: "DELETE",
      body: "{}",
    });
  },
  runSchedule(id: string): Promise<{ started: boolean }> {
    return request(`/api/schedules/${encodeURIComponent(id)}/run`, {
      method: "POST",
      body: "{}",
    });
  },

  sendSmsCode(phone: string): Promise<SmsCodeResult> {
    return request("/api/auth/sms", { method: "POST", body: jsonBody({ phone }) });
  },
  loginWithCode(code: string): Promise<LoginResult> {
    return request("/api/auth/login-code", { method: "POST", body: jsonBody({ code }) });
  },
  logoutAccount(key?: string): Promise<Account[]> {
    return request("/api/accounts/logout", {
      method: "POST",
      body: jsonBody(key ? { key } : {}),
    });
  },
  setActiveAccount(key: string): Promise<Account[]> {
    return request("/api/accounts", { method: "POST", body: jsonBody({ key }) });
  },

  getHistory(): Promise<HistoryDay[]> {
    return request<HistoryDay[]>("/api/history");
  },
  getLogs(limit = 500): Promise<LogPage> {
    return request<LogPage>(`/api/logs?limit=${limit}`);
  },

  async exportCredentials(): Promise<TransferResult> {
    const res = await fetch("/api/export", {
      credentials: "same-origin",
      headers: { [CSRF_HEADER]: "1" },
    });
    if (!res.ok) {
      let message = res.statusText;
      try {
        const body = (await res.json()) as { error?: string };
        if (body.error) message = body.error;
      } catch {
        // ignore
      }
      return { cancelled: false, succeeded: false, message };
    }
    const blob = await res.blob();
    const disposition = res.headers.get("Content-Disposition") || "";
    const match = /filename="([^"]+)"/.exec(disposition);
    const filename = match?.[1] || "glacc-accounts.json";
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
    return { cancelled: false, succeeded: true, message: `已导出到 ${filename}` };
  },

  async importCredentials(): Promise<TransferResult> {
    const file = await pickJsonFile();
    if (!file) return { cancelled: true, succeeded: false, message: "已取消导入" };
    const text = await file.text();
    try {
      const result = await request<{ imported: string[]; skipped: [string, string][] }>(
        "/api/import",
        { method: "POST", body: text },
      );
      const skipped = result.skipped.length;
      return {
        cancelled: false,
        succeeded: true,
        message: `已导入 ${result.imported.length} 个账号${skipped ? `，跳过 ${skipped} 个` : ""}`,
      };
    } catch (e) {
      return { cancelled: false, succeeded: false, message: String(e) };
    }
  },

  /** 兼容旧设置页：定时任务已改为独立的 /api/schedules，这里只返回空状态。 */
  applySchedule(enabled: boolean, time: string) {
    return Promise.resolve({
      enabled,
      time,
      exists: false,
      succeeded: false,
      message: "定时任务已迁移到「定时」页",
    });
  },

  openExternal(url: string): Promise<void> {
    window.open(url, "_blank", "noopener,noreferrer");
    return Promise.resolve();
  },
};

function pickJsonFile(): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "application/json,.json";
    input.onchange = () => resolve(input.files?.[0] ?? null);
    input.oncancel = () => resolve(null);
    input.click();
  });
}

export type WindowAction = "minimize" | "toggleMaximize" | "close";
export function windowControl(_action: WindowAction): void {}
export function windowDrag(): void {}
export function windowShow(): void {}
