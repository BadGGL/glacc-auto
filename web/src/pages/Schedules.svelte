<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../api";
  import { appStore } from "../stores.svelte";
  import type { Account, ScheduleRule } from "../types";
  import Button from "../lib/components/ui/button/button.svelte";
  import Input from "../lib/components/ui/input/input.svelte";
  import Select from "../lib/components/ui/select/select.svelte";
  import { Play, Plus, RefreshCw, Trash2 } from "@lucide/svelte";

  let rules = $state<ScheduleRule[]>([]);
  let accounts = $state<Account[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state("");

  let name = $state("");
  let cron = $state("0 8 * * *");
  let timezone = $state("Asia/Shanghai");
  let scope = $state("all");
  let catchUp = $state(true);
  let jitter = $state("0");

  const scopeOptions = $derived([
    { value: "all", label: "全部账号" },
    ...accounts.map((a) => ({ value: a.key, label: a.label })),
  ]);

  async function refresh(): Promise<void> {
    loading = true;
    error = "";
    try {
      [rules, accounts] = await Promise.all([api.getSchedules(), api.getAccounts()]);
      appStore.schedules = rules;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void refresh();
  });

  async function create(): Promise<void> {
    if (busy) return;
    busy = true;
    error = "";
    try {
      await api.createSchedule({
        id: "",
        name: name.trim(),
        cron: cron.trim(),
        timezone: timezone.trim(),
        scope,
        enabled: true,
        catchUp,
        jitterSecs: Math.max(0, Number.parseInt(jitter, 10) || 0),
        createdAt: 0,
        lastRunAt: null,
        lastStatus: null,
      });
      name = "";
      await refresh();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function toggle(rule: ScheduleRule): Promise<void> {
    try {
      await api.updateSchedule(rule.id, { ...rule, enabled: !rule.enabled });
      await refresh();
    } catch (e) {
      error = String(e);
    }
  }

  async function remove(rule: ScheduleRule): Promise<void> {
    if (!confirm(`删除定时任务「${rule.name}」？`)) return;
    try {
      await api.deleteSchedule(rule.id);
      await refresh();
    } catch (e) {
      error = String(e);
    }
  }

  async function run(rule: ScheduleRule): Promise<void> {
    try {
      await api.runSchedule(rule.id);
      await refresh();
    } catch (e) {
      error = String(e);
    }
  }

  function describe(rule: ScheduleRule): string {
    const who = rule.scope === "all" ? "全部账号" : accounts.find((a) => a.key === rule.scope)?.label ?? rule.scope;
    return `${rule.cron} · ${rule.timezone} · ${who}`;
  }
</script>

<section class="soft-scrollbar mx-auto flex h-full min-h-0 w-full max-w-[760px] flex-col gap-5 px-8 pb-16 pt-6">
  <div class="flex items-center justify-between gap-4">
    <div>
      <h2 class="text-xl font-bold text-card-foreground">定时任务</h2>
      <p class="mt-1 text-sm text-muted-foreground">服务进程内执行，支持多条 cron 规则与错峰抖动。</p>
    </div>
    <Button variant="outline" size="sm" onclick={() => void refresh()} disabled={loading}>
      <RefreshCw class="mr-1 size-4" />刷新
    </Button>
  </div>

  {#if error}
    <div class="rounded-btn bg-destructive/10 px-4 py-3 text-sm leading-relaxed text-destructive">
      {error}
    </div>
  {/if}

  <div class="space-y-3 rounded-card border border-border bg-card p-5">
    <div class="text-base font-semibold text-card-foreground">新建规则</div>
    <div class="grid gap-3 sm:grid-cols-2">
      <label class="space-y-1 text-sm">
        <span class="text-muted-foreground">名称</span>
        <Input class="h-11 w-full px-3" placeholder="每天早八点" bind:value={name} />
      </label>
      <label class="space-y-1 text-sm">
        <span class="text-muted-foreground">cron（分 时 日 月 周）</span>
        <Input class="h-11 w-full px-3 font-mono" placeholder="0 8 * * *" bind:value={cron} />
      </label>
      <label class="space-y-1 text-sm">
        <span class="text-muted-foreground">时区</span>
        <Input class="h-11 w-full px-3 font-mono" placeholder="Asia/Shanghai" bind:value={timezone} />
      </label>
      <label class="space-y-1 text-sm">
        <span class="text-muted-foreground">范围</span>
        <Select class="h-11 w-full" value={scope} options={scopeOptions} onValueChange={(v) => (scope = v)} />
      </label>
      <label class="space-y-1 text-sm">
        <span class="text-muted-foreground">随机抖动（秒）</span>
        <Input class="h-11 w-full px-3" inputmode="numeric" bind:value={jitter} />
      </label>
      <label class="flex items-center gap-2 pt-6 text-sm text-muted-foreground">
        <input type="checkbox" bind:checked={catchUp} class="size-4 accent-primary" />
        停机错过后补跑
      </label>
    </div>
    <Button onclick={() => void create()} disabled={busy || name.trim() === ""}>
      <Plus class="mr-1 size-4" />创建
    </Button>
  </div>

  <div class="space-y-2">
    {#if loading}
      <div class="py-10 text-center text-sm text-muted-foreground">正在加载…</div>
    {:else if rules.length === 0}
      <div class="rounded-card border border-dashed border-border py-12 text-center text-sm text-muted-foreground">
        还没有定时任务
      </div>
    {:else}
      {#each rules as rule (rule.id)}
        <div class="flex items-center justify-between gap-4 rounded-card border border-border bg-card p-4">
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <span class="truncate text-base font-medium text-card-foreground">{rule.name}</span>
              <span
                class="rounded-full px-2 py-0.5 text-xs {rule.enabled
                  ? 'bg-emerald-500/15 text-emerald-600'
                  : 'bg-muted text-muted-foreground'}"
              >
                {rule.enabled ? "启用" : "停用"}
              </span>
            </div>
            <div class="mt-1 truncate font-mono text-xs text-muted-foreground">{describe(rule)}</div>
            {#if rule.lastStatus}
              <div class="mt-1 text-xs text-muted-foreground">最近：{rule.lastStatus}</div>
            {/if}
          </div>
          <div class="flex shrink-0 items-center gap-1">
            <Button variant="ghost" size="sm" class="size-8 px-0" title="立即执行" onclick={() => void run(rule)}>
              <Play class="size-4" />
            </Button>
            <Button variant="ghost" size="sm" onclick={() => void toggle(rule)}>
              {rule.enabled ? "停用" : "启用"}
            </Button>
            <Button variant="ghost" size="sm" class="size-8 px-0" title="删除" onclick={() => void remove(rule)}>
              <Trash2 class="size-4" />
            </Button>
          </div>
        </div>
      {/each}
    {/if}
  </div>
</section>
