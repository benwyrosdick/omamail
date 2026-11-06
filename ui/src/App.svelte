<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    applyPalette,
    FALLBACK,
    inTauri,
    loadTheme,
    onTheme,
    type InboxItem,
    type MessageView,
    type Status,
  } from "./lib/api";

  let status = $state<Status>({
    signed_in: false,
    email: null,
    client_configured: false,
    error: null,
  });
  let messages = $state<InboxItem[]>([]);
  let selected = $state(0);
  let open = $state<MessageView | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let desktop = $state(true);

  const current = $derived(messages[selected] ?? null);

  onMount(() => {
    desktop = inTauri();
    applyPalette(FALLBACK);
    let unlisten = () => {};
    void (async () => {
      try {
        applyPalette(await loadTheme());
        if (desktop) {
          const stop = await onTheme(applyPalette);
          unlisten = () => {
            void stop();
          };
          await refresh();
          if (status.signed_in && messages.length === 0) {
            await sync();
          }
        }
      } catch (err) {
        error = String(err);
      }
    })();
    return () => unlisten();
  });

  async function refresh() {
    status = await api.status();
    error = status.error;
    if (status.signed_in) {
      messages = await api.inbox();
      if (selected >= messages.length) selected = 0;
      if (current) await openCurrent();
      else open = null;
    } else {
      messages = [];
      open = null;
    }
  }

  async function connect() {
    busy = "Signing in and fetching mail…";
    error = null;
    try {
      await api.login();
      await refresh();
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
    }
  }

  async function disconnect() {
    busy = "Signing out…";
    error = null;
    try {
      await api.logout();
      await refresh();
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
    }
  }

  async function sync() {
    if (!status.signed_in || busy) return;
    busy = "Syncing inbox…";
    error = null;
    try {
      await api.sync();
      messages = await api.inbox();
      if (selected >= messages.length) selected = 0;
      if (current) await openCurrent();
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
    }
  }

  async function openCurrent() {
    if (!current) return;
    try {
      open = await api.message(current.id);
    } catch (err) {
      error = String(err);
    }
  }

  function move(delta: number) {
    if (messages.length === 0) return;
    selected = Math.min(messages.length - 1, Math.max(0, selected + delta));
    void openCurrent();
  }

  function onKey(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target as HTMLElement | null;
    if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;
    if (event.key === "j" || event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "k" || event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      void openCurrent();
    } else if (event.key === "r") {
      event.preventDefault();
      void sync();
    }
  }

  function mailDocument(html: string): string {
    return `<!doctype html><html><head><meta charset="utf-8"><base target="_blank"><style>
      html,body{margin:0;background:#fff;color:#1b1b1f;}
      body{padding:16px 18px 32px;font:15px/1.5 Inter,system-ui,sans-serif;}
      img,video{max-width:100%;height:auto;}
      a{color:#3566d6;}
      table{max-width:100%;}
    </style></head><body>${html}</body></html>`;
  }

  function when(ms: number | null): string {
    if (ms == null) return "";
    const date = new Date(ms);
    const now = new Date();
    if (date.toDateString() === now.toDateString()) {
      return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    }
    return date.toLocaleDateString([], { month: "short", day: "numeric" });
  }
</script>

<svelte:window onkeydown={onKey} />

<main class="app">
  <aside class="rail">
    <div class="mark">
      <span class="seal" aria-hidden="true"></span>
      <span class="word">OmaMail</span>
    </div>
    <div class="sub">Gmail</div>
    <div class="email">{status.email ?? "Not signed in"}</div>
    <div class="actions">
      {#if status.signed_in}
        <button class="primary" disabled={!!busy} onclick={sync}>Sync</button>
        <button disabled={!!busy} onclick={disconnect}>Sign out</button>
      {:else}
        <button class="primary" disabled={!!busy || !desktop} onclick={connect}>
          Connect Google
        </button>
      {/if}
    </div>
    {#if busy}<p class="busy">{busy}</p>{/if}
    {#if error}<p class="error">{error}</p>{/if}
    <dl class="hint">
      <div><dt>j k</dt><dd>Up / down</dd></div>
      <div><dt>enter</dt><dd>Open</dd></div>
      <div><dt>r</dt><dd>Refresh</dd></div>
    </dl>
  </aside>

  <section class="list">
    <header class="list-head">
      <h1>Inbox</h1>
      <span class="count">{messages.length ? `${messages.length}` : ""}</span>
    </header>
    {#if messages.length === 0}
      <p class="empty">
        {#if !desktop}
          This window is the shell. Run <code>npm run dev</code> from the repo to open it in OmaMail.
        {:else if !status.signed_in}
          Connect a Google account. Mail stays on this machine — tokens in the keyring, headers in a local database.
        {:else}
          Nothing cached yet. Sync pulls the latest 50 inbox messages.
        {/if}
      </p>
    {:else}
      <div class="rows">
        {#each messages as message, index (message.id)}
          <button
            class="row"
            class:selected={index === selected}
            class:unread={message.unread}
            onclick={() => {
              selected = index;
              void openCurrent();
            }}
          >
            <span class="subject">{message.subject || "(no subject)"}</span>
            <span class="when">{when(message.date_ms)}</span>
            <span class="from">{message.from}</span>
            <span class="snippet">{message.snippet}</span>
          </button>
        {/each}
      </div>
    {/if}
  </section>

  <section class="reader">
    {#if open}
      <header class="reader-head">
        <h1>{open.subject || "(no subject)"}</h1>
      </header>
      <div class="meta">{open.from} · {when(open.date_ms)}</div>
      {#if open.html}
        <iframe
          class="html-mail"
          title="Message"
          sandbox="allow-popups allow-popups-to-escape-sandbox"
          srcdoc={mailDocument(open.html)}
        ></iframe>
      {:else}
        <pre class="body">{open.plain_text}</pre>
      {/if}
    {:else}
      <p class="empty">Select a message.</p>
    {/if}
  </section>
</main>
