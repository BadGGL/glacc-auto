<script lang="ts">
  // Web 外壳：密码登录门 + 左侧导航 + 页面区 + 全局弹窗/轻提示。
  // 不再有 Tauri 标题栏与窗口控制；页面路由仍由 appStore.page 区分。
  import { onMount } from "svelte";
  import NavRail from "./src/lib/NavRail.svelte";
  import Toast from "./src/lib/Toast.svelte";
  import Home from "./src/pages/Home.svelte";
  import Settings from "./src/pages/Settings.svelte";
  import Schedules from "./src/pages/Schedules.svelte";
  import LoginDialog from "./src/lib/Dialogs/LoginDialog.svelte";
  import ConfirmDialog from "./src/lib/Dialogs/ConfirmDialog.svelte";
  import Modal from "./src/lib/Dialogs/Modal.svelte";
  import Button from "./src/lib/components/ui/button/button.svelte";
  import Input from "./src/lib/components/ui/input/input.svelte";
  import { Tooltip } from "bits-ui";
  import { appStore } from "./src/stores.svelte";
  import { api } from "./src/api";

  const DISCLAIMER_KEY = "hasSeenDisclaimer";
  let hasSeenDisclaimer = $state(localStorage.getItem(DISCLAIMER_KEY) === "true");

  let authChecked = $state(false);
  let authed = $state(false);
  let initialized = $state(false);
  let password = $state("");
  let loginBusy = $state(false);
  let loginError = $state("");

  function acceptDisclaimer(): void {
    localStorage.setItem(DISCLAIMER_KEY, "true");
    hasSeenDisclaimer = true;
  }

  async function checkAuth(): Promise<void> {
    try {
      const state = await api.authState();
      authed = state.authenticated;
    } catch {
      authed = false;
    } finally {
      authChecked = true;
    }
    if (authed) await enterApp();
  }

  async function enterApp(): Promise<void> {
    if (initialized) return;
    initialized = true;
    await appStore.init().catch(() => {
      // init 内部已把错误写进 errorText
    });
  }

  async function submitLogin(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (loginBusy || password.trim() === "") return;
    loginBusy = true;
    loginError = "";
    try {
      await api.login(password);
      password = "";
      authed = true;
      await enterApp();
    } catch (err) {
      loginError = String(err);
    } finally {
      loginBusy = false;
    }
  }

  async function logout(): Promise<void> {
    try {
      await api.logout();
    } finally {
      authed = false;
      initialized = false;
      appStore.stopPolling();
    }
  }

  onMount(() => {
    void checkAuth();
    return () => {
      appStore.stopPolling();
      appStore.stopResendTimer();
    };
  });
</script>

{#if !authChecked}
  <div class="grid h-screen place-items-center bg-background text-muted-foreground">正在加载…</div>
{:else if !authed}
  <div class="grid min-h-screen place-items-center bg-background px-6">
    <form
      class="w-full max-w-sm space-y-5 rounded-card border border-border bg-card p-8 shadow-sm"
      onsubmit={submitLogin}
    >
      <div class="space-y-1 text-center">
        <h1 class="text-2xl font-bold text-card-foreground">glacc-auto</h1>
        <p class="text-sm text-muted-foreground">请输入网页访问密码</p>
      </div>
      <Input
        type="password"
        class="h-12 w-full px-3 text-base"
        placeholder="访问密码"
        bind:value={password}
        autocomplete="current-password"
        aria-label="访问密码"
      />
      {#if loginError}
        <div class="rounded-btn bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {loginError}
        </div>
      {/if}
      <Button class="h-12 w-full" type="submit" disabled={loginBusy || password.trim() === ""}>
        {loginBusy ? "登录中…" : "登录"}
      </Button>
      <p class="text-center text-xs leading-relaxed text-muted-foreground">
        首次启动的密码见服务端 stderr，或通过 GLACC_PASSWORD 环境变量指定。
      </p>
    </form>
  </div>
{:else}
  <div class="flex h-screen flex-col overflow-hidden">
    <Tooltip.Provider delayDuration={400}>
      <div class="flex min-h-0 flex-1">
        <NavRail onlogout={() => void logout()} />
        <main class="relative min-w-0 flex-1 overflow-hidden">
          {#if appStore.page === "home"}
            <Home />
          {:else if appStore.page === "schedules"}
            <Schedules />
          {:else}
            <Settings />
          {/if}
        </main>
      </div>
    </Tooltip.Provider>
  </div>

  <div class="pointer-events-none fixed inset-x-0 bottom-5 z-[80] flex justify-center">
    <Toast />
  </div>

  <LoginDialog />
  <ConfirmDialog
    open={appStore.logoutConfirmOpen}
    zIndex={65}
    title="退出登录"
    body="退出后将清除本机保存的手机号与登录信息，需重新用短信验证码登录。"
    confirmText="退出登录"
    cancelText="取消"
    danger
    busy={appStore.logoutBusy}
    onconfirm={() => void appStore.confirmLogout()}
    oncancel={() => appStore.cancelLogout()}
  />

  <Modal
    open={!hasSeenDisclaimer}
    zIndex={100}
    title="免责声明"
    closable={false}
    description="本项目仅供个人学习、研究与技术交流，严禁任何商业或盈利性使用。本软件为非官方第三方工具，与任何第三方服务提供方无任何关联。使用可能带来的账号限制、权益损失及法律风险由使用者自行承担。"
  >
    <div class="mt-6">
      <Button class="w-full" onclick={acceptDisclaimer}>我已阅读并同意</Button>
    </div>
  </Modal>
{/if}
