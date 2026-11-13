<script lang="ts">
  // 设置页（步 7 由原设置浮窗 SettingsModal 改为独立页面）：
  // 内容与浮窗完全一致，外壳换成页面容器，不再有遮罩/Esc/关闭按钮。
  import { appStore } from "../stores.svelte";
  import { api } from "../api";
  import Input from "../lib/components/ui/input/input.svelte";
  import Select from "../lib/components/ui/select/select.svelte";
  import { ExternalLink } from "@lucide/svelte";

  const settings = $derived(appStore.settings);

  // 领取间隔：本地文本态，失焦时夹紧回写（旧版 CoerceIntervalInput）
  let intervalMinText = $state("30");
  let intervalMaxText = $state("40");

  $effect(() => {
    intervalMinText = String(settings.intervalMinSec);
    intervalMaxText = String(settings.intervalMaxSec);
  });

  const retryOptions = Array.from({ length: 11 }, (_, i) => ({
    value: String(i),
    label: `${i} 次`,
  }));

  /** 间隔输入钳制：数字、1~3600、保持 min ≤ max（越界一侧夹到另一侧）。 */
  function coerceInterval(text: string, isMin: boolean): number {
    const current = isMin ? settings.intervalMinSec : settings.intervalMaxSec;
    const other = isMin ? settings.intervalMaxSec : settings.intervalMinSec;
    let n = Number.parseInt(text, 10);
    if (!Number.isFinite(n)) return current;
    n = Math.min(3600, Math.max(1, n));
    if (isMin && n > other) n = other;
    if (!isMin && n < other) n = other;
    return n;
  }

  function commitInterval(isMin: boolean): void {
    const n = coerceInterval(isMin ? intervalMinText : intervalMaxText, isMin);
    if (isMin) {
      intervalMinText = String(n);
      if (n !== settings.intervalMinSec) void appStore.saveSettings({ intervalMinSec: n });
    } else {
      intervalMaxText = String(n);
      if (n !== settings.intervalMaxSec) void appStore.saveSettings({ intervalMaxSec: n });
    }
  }

</script>

<!-- 页面自身锁高（h-full 由父级 main 的 overflow-hidden 保证），
     所有滚动只发生在这个容器内部；pb-16 保证 GitHub 图标不被裁切 -->
<section class="soft-scrollbar mx-auto flex h-full min-h-0 w-full max-w-[680px] flex-col gap-5 px-8 pb-16 pt-6">
  <!-- 领取 -->
  <div class="space-y-3">
    <h3 class="text-xl font-bold text-card-foreground">领取</h3>

    <div class="flex items-center justify-between gap-4 rounded-card border border-border bg-card p-6">
      <div class="min-w-0">
        <div class="text-lg text-card-foreground">领取间隔</div>
        <div class="mt-1 text-sm leading-snug text-muted-foreground">
          每次动作的间隔，在此区间内随机取值
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <Input
          class="h-12 w-20 px-2 text-center text-base tabular-nums"
          inputmode="numeric"
          maxlength="4"
          bind:value={intervalMinText}
          onblur={() => commitInterval(true)}
          aria-label="领取间隔下限（秒）"
        />
        <span class="text-base text-muted-foreground">~</span>
        <Input
          class="h-12 w-20 px-2 text-center text-base tabular-nums"
          inputmode="numeric"
          maxlength="4"
          bind:value={intervalMaxText}
          onblur={() => commitInterval(false)}
          aria-label="领取间隔上限（秒）"
        />
        <span class="text-sm text-muted-foreground">秒</span>
      </div>
    </div>

    <div class="flex items-center justify-between gap-4 rounded-card border border-border bg-card p-6">
      <div class="min-w-0">
        <div class="text-lg text-card-foreground">网络重试次数</div>
        <div class="mt-1 text-sm leading-snug text-muted-foreground">
          失败后的自动重试，0 为立即中断
        </div>
      </div>
      <Select
        class="h-12 w-[120px] text-base"
        value={String(settings.networkRetryCount)}
        options={retryOptions}
        aria-label="网络重试次数"
        onValueChange={(v) => void appStore.saveSettings({ networkRetryCount: Number(v) })}
      />
    </div>

    <button
      class="flex w-full items-center justify-between gap-4 rounded-card border border-border bg-card p-6 text-left transition-colors hover:bg-accent/40"
      onclick={() => (appStore.page = "schedules")}
    >
      <div class="min-w-0">
        <div class="text-lg text-card-foreground">定时任务</div>
        <div class="mt-1 text-sm leading-snug text-muted-foreground">
          已迁移到独立页面，可配置多条 cron、时区与错开执行
        </div>
      </div>
      <span class="shrink-0 text-sm text-primary">前往配置</span>
    </button>
  </div>

  {#if appStore.settingsError}
    <div class="rounded-btn bg-destructive/10 px-4 py-3 text-sm leading-relaxed text-destructive">
      {appStore.settingsError}
    </div>
  {/if}

  <!-- 底部免责声明（定时领取的成功提示已改为 Toast，不再占页面布局） -->
  <div class="mt-auto flex justify-center px-2 pt-4">
    <p class="text-center text-xs font-semibold leading-relaxed text-red-500/80">
      本项目仅供个人学习、研究与技术交流，严禁任何商业或盈利性使用。本软件为非官方第三方工具，与任何第三方服务提供方无任何关联。使用可能带来的账号限制、权益损失及法律风险由使用者自行承担。
    </p>
  </div>
</section>
