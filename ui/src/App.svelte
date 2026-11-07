<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    applyPalette,
    FALLBACK,
    inTauri,
    loadTheme,
    onTheme,
    type Category,
    type InboxItem,
    type MailSettings,
    type MessageView,
    type Status,
  } from "./lib/api";
  import type { Action } from "svelte/action";

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
  let settingsOpen = $state(false);
  let mailSettings = $state<MailSettings | null>(null);
  let apiKeyDraft = $state("");
  let jevEnabled = $state(false);
  let categoryDraft = $state<Category[]>([]);
  let categoryFilter = $state("all");
  let dragIndex = $state<number | null>(null);
  let moveOpen = $state(false);
  let folders = $state<{ id: string; name: string }[]>([]);

  const visible = $derived(
    categoryFilter === "all"
      ? messages
      : messages.filter((message) => message.category === categoryFilter),
  );
  const current = $derived(visible[selected] ?? null);

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
          await loadSettings();
          await refresh();
          if (status.signed_in && messages.length === 0) {
            await sync();
          } else if (jevEnabled && mailSettings?.key_configured && messages.some((message) => !message.category)) {
            await sortMail();
          }
        }
      } catch (err) {
        error = String(err);
      }
    })();
    return () => unlisten();
  });

  async function loadSettings() {
    mailSettings = await api.settings();
    jevEnabled = mailSettings.jev_enabled;
    categoryDraft = mailSettings.categories.map((category) => ({ ...category }));
  }

  async function refresh() {
    status = await api.status();
    error = status.error;
    if (status.signed_in) {
      messages = await api.inbox();
      if (selected >= visible.length) selected = 0;
      if (current) await openCurrent();
      else open = null;
    } else {
      messages = [];
      open = null;
    }
  }

  function moveCategory(from: number, to: number) {
    if (from === to) return;
    const next = [...categoryDraft];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    categoryDraft = next;
  }

  async function sortMail(all = false) {
    busy = all ? "Re-sorting every message…" : "Sorting with Jev…";
    error = null;
    try {
      await api.classify(all);
      messages = await api.inbox();
      if (selected >= visible.length) selected = 0;
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
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
      if (jevEnabled && mailSettings?.key_configured) {
        busy = "Sorting with Jev…";
        await api.classify();
        messages = await api.inbox();
      }
      if (selected >= visible.length) selected = 0;
      if (current) await openCurrent();
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
    }
  }

  async function openCurrent() {
    if (!current) return;
    const id = current.id;
    const wasUnread = current.unread;
    try {
      open = await api.message(id);
      if (wasUnread) {
        messages = messages.map((message) =>
          message.id === id ? { ...message, unread: false } : message,
        );
        void api.act("read", id).catch((err) => {
          error = String(err);
          messages = messages.map((message) =>
            message.id === id ? { ...message, unread: true } : message,
          );
        });
      }
    } catch (err) {
      error = String(err);
    }
  }

  function move(delta: number) {
    if (visible.length === 0) return;
    selected = Math.min(visible.length - 1, Math.max(0, selected + delta));
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
    } else if (event.key === "r" && !event.shiftKey) {
      event.preventDefault();
      void sync();
    } else if (settingsOpen || !current) {
      return;
    } else if (event.key === "e") {
      event.preventDefault();
      void mailAction("archive");
    } else if (event.key === "#" || event.key === "Delete") {
      event.preventDefault();
      void mailAction("trash");
    } else if (event.key === "!") {
      event.preventDefault();
      void mailAction("spam");
    } else if (event.key === "v") {
      event.preventDefault();
      void toggleMove();
    }
  }

  async function toggleMove() {
    moveOpen = !moveOpen;
    if (!moveOpen) return;
    try {
      folders = await api.folders();
    } catch (err) {
      error = String(err);
      moveOpen = false;
    }
  }

  async function mailAction(kind: string, labelId?: string) {
    if (!current || busy) return;
    const id = current.id;
    const index = selected;
    busy = kind === "read" ? "Marking read…" : "Updating Gmail…";
    error = null;
    try {
      await api.act(kind, id, labelId);
      if (kind === "read") {
        messages = messages.map((message) =>
          message.id === id ? { ...message, unread: false } : message,
        );
        if (open?.id === id) open = { ...open, unread: false };
      } else {
        messages = messages.filter((message) => message.id !== id);
        open = null;
        selected = Math.max(0, Math.min(index, Math.max(visible.length - 1, 0)));
        if (current) await openCurrent();
      }
      moveOpen = false;
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
    }
  }

  const renderMail: Action<HTMLElement, string> = (node, html) => {
    const root = node.shadowRoot ?? node.attachShadow({ mode: "open" });
    const paint = (body: string) => {
      root.innerHTML = `<style>
        :host { display: block; min-height: 100%; background: #fff; color: #1b1b1f; }
        .sheet { padding: 16px 18px 32px; font: 15px/1.5 Inter, system-ui, sans-serif; }
        img, video { max-width: 100%; height: auto; }
        a { color: #3566d6; cursor: pointer; }
        table { max-width: 100%; }
        .preheader, .preview-text, #preheader { display: none !important; }
      </style><div class="sheet">${body}</div>`;
    };
    const onClick = (event: Event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      const anchor = target.closest("a");
      const href = anchor?.getAttribute("href");
      if (!href) return;
      event.preventDefault();
      void api.openLink(href).catch((err) => {
        error = String(err);
      });
    };
    paint(html);
    root.addEventListener("click", onClick);
    return {
      update(next) {
        paint(next);
      },
      destroy() {
        root.removeEventListener("click", onClick);
      },
    };
  };

  async function openSettings() {
    settingsOpen = true;
    error = null;
    try {
      await loadSettings();
      apiKeyDraft = "";
    } catch (err) {
      error = String(err);
    }
  }

  async function saveSettings() {
    busy = "Saving settings…";
    error = null;
    try {
      mailSettings = await api.saveSettings({
        api_key: apiKeyDraft.trim() ? apiKeyDraft.trim() : null,
        jev_enabled: jevEnabled,
        categories: categoryDraft,
      });
      jevEnabled = mailSettings.jev_enabled;
      categoryDraft = mailSettings.categories.map((category) => ({ ...category }));
      apiKeyDraft = "";
      if (jevEnabled && mailSettings.key_configured && status.signed_in) {
        await sortMail(true);
      }
    } catch (err) {
      error = String(err);
    } finally {
      busy = null;
    }
  }

  function addCategory() {
    categoryDraft = [...categoryDraft, { name: "", description: "", color: "#89b4fa" }];
  }

  function removeCategory(index: number) {
    categoryDraft = categoryDraft.filter((_, i) => i !== index);
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
  <section class="list">
    <header class="list-head">
      <h1>Inbox</h1>
      <span class="count">{visible.length ? `${visible.length}` : ""}</span>
    </header>
    {#if mailSettings && (jevEnabled || messages.some((message) => message.category))}
      <div class="filters">
        <button class:on={categoryFilter === "all"} onclick={() => (categoryFilter = "all")}>All</button>
        {#each mailSettings.categories as category (category.name)}
          <button
            class:on={categoryFilter === category.name}
            onclick={() => {
              categoryFilter = category.name;
              selected = 0;
            }}>{category.name}</button
          >
        {/each}
      </div>
    {/if}
    {#if messages.length === 0}
      <p class="empty">
        {#if !desktop}
          This window is the shell. Run <code>npm run dev</code> from the repo to open it in OmaMail.
        {:else if !status.signed_in}
          Connect a Google account. Mail stays on this machine — tokens in the keyring, headers in a local database.
        {:else}
          Nothing cached yet. Refresh pulls the latest 50 inbox messages.
        {/if}
      </p>
    {:else}
      <div class="rows">
        {#each visible as message, index (message.id)}
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
            {#if message.tags.length}
              <span class="tags">
                {#each message.tags as tag (tag.kind + tag.name)}
                  <span class="tag" class:jev={tag.kind === "jev"}>
                    <i style={tag.color ? `background:${tag.color}` : ""}></i>{tag.name}
                  </span>
                {/each}
              </span>
            {/if}
            <span class="snippet">{message.snippet}</span>
          </button>
        {/each}
      </div>
    {/if}
    <div class="account">
      <span class="seal" aria-hidden="true"></span>
      <div class="account-copy">
        <div class="email">{status.email ?? "Not signed in"}</div>
        {#if busy}<div class="busy">{busy}</div>{:else if error}<div class="error">{error}</div>{/if}
      </div>
      {#if status.signed_in}
        <button class="primary" disabled={!!busy} onclick={sync}>Refresh</button>
        <button disabled={!!busy} onclick={disconnect}>Sign out</button>
      {:else}
        <button class="primary" disabled={!!busy || !desktop} onclick={connect}>Connect</button>
      {/if}
      <button disabled={!!busy} onclick={openSettings}>Settings</button>
    </div>
  </section>

  <section class="reader">
    {#if settingsOpen}
      <header class="reader-head">
        <h1>Settings</h1>
        <button onclick={() => (settingsOpen = false)}>Close</button>
      </header>
      <form class="settings" onsubmit={(event) => { event.preventDefault(); void saveSettings(); }}>
        <label>
          TypeSafe API key
          <input
            type="password"
            autocomplete="off"
            placeholder={mailSettings?.key_configured ? "Saved — paste to replace" : "Paste your key"}
            bind:value={apiKeyDraft}
          />
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={jevEnabled} />
          Sort mail with Jev
        </label>
        <div class="categories">
          <div class="categories-head">
            <h2>Categories</h2>
            <button type="button" onclick={addCategory}>Add</button>
          </div>
          {#each categoryDraft as category, index (index)}
            <div
              class="category-edit"
              class:dragging={dragIndex === index}
              ondragover={(event) => event.preventDefault()}
              ondrop={() => {
                if (dragIndex !== null) moveCategory(dragIndex, index);
                dragIndex = null;
              }}
            >
              <span
                class="handle"
                draggable="true"
                title="Drag to reorder"
                aria-label="Drag to reorder"
                ondragstart={() => (dragIndex = index)}
                ondragend={() => (dragIndex = null)}>⠿</span
              >
              <input type="color" bind:value={category.color} aria-label="Color for {category.name}" />
              <input class="name" placeholder="Name" bind:value={category.name} />
              <input class="description" placeholder="What belongs here" bind:value={category.description} />
              <button type="button" onclick={() => removeCategory(index)}>Remove</button>
            </div>
          {/each}
        </div>
        <div class="settings-actions">
          <button class="primary" type="submit" disabled={!!busy}>Save</button>
          <button type="button" disabled={!!busy || !status.signed_in} onclick={() => sortMail(true)}>Sort inbox</button>
        </div>
        {#if error}<p class="error">{error}</p>{/if}
      </form>
    {:else if open}
      <header class="reader-head">
        <h1>{open.subject || "(no subject)"}</h1>
      </header>
      {#if open.tags.length}
        <div class="preview-tags">
          {#each open.tags as tag (tag.kind + tag.name)}
            <span class="tag" class:jev={tag.kind === "jev"}>
              <i style={tag.color ? `background:${tag.color}` : ""}></i>{tag.name}
            </span>
          {/each}
        </div>
      {/if}
      <div class="meta">{open.from} · {when(open.date_ms)}</div>
      <div class="action-bar">
        <button title="Archive (e)" disabled={!!busy} onclick={() => mailAction("archive")}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16v11H4z"/><path d="M4 7l8 6 8-6"/><path d="M12 3v8"/><path d="M9 8l3 3 3-3"/></svg>
        </button>
        <button title="Report spam (!)" disabled={!!busy} onclick={() => mailAction("spam")}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3l8 3v6c0 5-3.4 8.4-8 9-4.6-.6-8-4-8-9V6z"/><path d="M12 8v5"/><path d="M12 16h.01"/></svg>
        </button>
        <button title="Delete (#)" disabled={!!busy} onclick={() => mailAction("trash")}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 7h14"/><path d="M9 7V5h6v2"/><path d="M8 7l1 12h6l1-12"/></svg>
        </button>
        <button title="Move (v)" disabled={!!busy} onclick={toggleMove}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 8h6l2 2h8v9H4z"/></svg>
        </button>
        {#if moveOpen}
          <div class="move-menu">
            {#each folders as folder (folder.id)}
              <button type="button" onclick={() => mailAction("move", folder.id)}>{folder.name}</button>
            {:else}
              <p>No folders yet. Refresh, then try again.</p>
            {/each}
          </div>
        {/if}
      </div>
      {#if open.html}
        <div class="html-mail" use:renderMail={open.html}></div>
      {:else}
        <pre class="body">{open.plain_text}</pre>
      {/if}
    {:else}
      <p class="empty">Select a message.</p>
    {/if}
  </section>
</main>
