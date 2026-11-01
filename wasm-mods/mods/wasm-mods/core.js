// The WASM Mods core screen (the `wasm-mods` mod opened on its own): the hosts installed
// and the one running, installing the core when none is, and the permissions: every
// installed WASM module, what it may do, and the answer
// you gave (allowed, denied, or asks). Deny, or forget an answer so the module
// asks again; turn a module off or on; remove one of yours.
//
// Nothing is decided here: wasm-mods-host lists and changes everything
// (`--permissions`, `--deny`, `--forget`, `--enable`, `--disable`, `--remove`), so
// permissions.toml stays the one place answers are kept and the host's rules
// apply. A capability is allowed only from the module's own prompt: Allow isn't
// offered here.
//
// Open it with `herdr-app do mod.open --name wasm-mods`, or bind a key in keys.json:
// "global": { "ctrl-alt-w": "mod.open:wasm-mods" }.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, Scrollbar } from "gpui-base";
import { exec, theme, props, closeOverlay, readTextFile, settings } from "herdr";
import { newestHost, installedHosts, hostsChanged, coreOffer, downloadHost, buildHost, parseRunFile, protocols, RUN_DIR } from "./wasm-bridge.js";

export class CoreScreen extends View {
  init(_props, cx) {
    this.inited = true;
    this.modules = null; // null: loading
    this.error = null;
    this.note = null;
    this.busy = false;
    this.confirm = null; // the module asked to be removed
    this.handle = cx.focus_handle();
    this.handle.focus();
    this.load(cx);
  }

  close() { closeOverlay(props()?.overlay); }

  async host(args) {
    const host = await newestHost();
    if (!host) throw new Error("no wasm-mods-host installed");
    const r = await exec(host.path, args);
    if (r.code !== 0) throw new Error((r.stderr || r.stdout || `wasm-mods-host exited ${r.code}`).trim().replace(/^wasm-mods-host: /, ""));
    return r.stdout;
  }

  load(cx) {
    cx.spawn(async (cx) => {
      hostsChanged();
      this.hosts = await installedHosts().catch(() => []);
      this.running = parseRunFile(await readTextFile(`${RUN_DIR()}/host`).catch(() => null));
      if (!this.hosts.length) {
        this.offer = await coreOffer("wasm-mods");
        this.modules = [];
        cx.notify();
        return;
      }
      this.offer = null;
      try {
        this.modules = JSON.parse(await this.host(["--permissions"]));
        this.error = null;
      } catch (e) {
        this.modules ??= [];
        this.error = `Can't read the modules: ${e.message ?? e}`;
      }
      cx.notify();
    });
  }

  /** Runs one change, then reads the list again (the host has the last word). */
  change(args, cx) {
    if (this.busy) return;
    this.busy = true;
    this.note = null;
    this.error = null;
    this.confirm = null;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const out = JSON.parse(await this.host(args));
        this.note = `${args[1]}: ${out.note}`;
      } catch (e) {
        this.error = String(e.message ?? e);
      }
      this.busy = false;
      this.load(cx);
    });
  }

  button(t, id, label, onClick, danger = false) {
    return div().id(id).px(10).py(3).rounded(6).border_1().border_color(t.border).bg(t.raised)
      .cursor_pointer().text_size(12.5).text_color(danger ? t.blocked : t.foreground)
      .hover((s) => s.bg(t.hover))
      .on_click(onClick)
      .child(label);
  }

  toggle(t, id, on, onClick) {
    return div().id(id).w(36).h(20).rounded(10).p(2).cursor_pointer()
      .bg(on ? t.accent : t.hover).flex().when(on, (el) => el.justify_end())
      .on_click(onClick)
      .child(div().w(16).h(16).rounded(8).bg(on ? t.accent_foreground : t.muted));
  }

  state(t, c) {
    if (!c.available) return { label: "not available", color: t.dim };
    if (c.state === "granted") return { label: "allowed", color: t.ansi2 };
    if (c.state === "denied") return { label: "denied", color: t.blocked };
    return { label: "asks", color: t.muted };
  }

  capability(t, m, c, i) {
    const id = `${m.name}-${i}`;
    const s = this.state(t, c);
    const run = (flag) => (_e, cx) => this.change([flag, m.name, c.id], cx);
    const actions = h_flex().gap(6).flex_none();
    if (c.changeable) {
      if (c.state !== "denied") actions.child(this.button(t, `${id}-deny`, "Deny", run("--deny")));
      if (c.state !== "ask") actions.child(this.button(t, `${id}-forget`, "Ask again", run("--forget")));
    }
    return h_flex().w_full().gap(12).py(6).items_center()
      .child(v_flex().flex_1().min_w(0).gap(2)
        .child(div().text_size(13).text_color(t.foreground).child(c.id))
        .child(div().text_size(12).line_height("17px").text_color(t.dim).child(c.about)))
      .child(div().flex_none().w(84).text_size(12.5).text_color(s.color).child(s.label))
      .child(actions);
  }

  module(t, m) {
    const key = m.name;
    const id = m.name;
    const row = v_flex().w_full().py(12).gap(4).border_b(1).border_color(t.border)
      .child(h_flex().gap(12).items_center()
        .child(v_flex().flex_1().min_w(0).gap(3)
          .child(h_flex().gap(8).items_center()
            .child(div().text_size(14).text_color(m.enabled ? t.foreground : t.muted).child(m.name))
            .when(!m.enabled, (el) => el.child(div().text_size(12).text_color(t.dim).child("· off"))))
          .when(Boolean(m.description), (el) => el.child(div().text_size(12.5).line_height("18px").text_color(t.dim).child(m.description))))
        .child(h_flex().flex_none().gap(10).items_center()
          .when(m.removable, (el) => el.child(this.button(t, `${id}-remove`, "Remove", (_e, cx) => { this.confirm = key; cx.notify(); })))
          .when(m.can_disable, (el) => el.child(this.toggle(t, `${id}-on`, m.enabled, (_e, cx) => this.change([m.enabled ? "--disable" : "--enable", m.name], cx))))));
    if (this.confirm === key) {
      row.child(v_flex().gap(8).py(6)
        .child(div().text_size(12.5).text_color(t.muted).child(`Remove ${m.name}? Its .wasm file is deleted and its answers forgotten.`))
        .child(h_flex().gap(6)
          .child(this.button(t, `${id}-cancel`, "Cancel", (_e, cx) => { this.confirm = null; cx.notify(); }))
          .child(this.button(t, `${id}-confirm`, `Remove ${m.name}`, (_e, cx) => this.change(["--remove", m.name], cx), true))));
    }
    if (m.capabilities.length === 0) {
      row.child(div().text_size(12.5).text_color(t.dim).child(m.has_manifest ? "Asks for nothing." : "No manifest: what it asks for shows up here once it asks."));
    } else {
      row.child(v_flex().w_full().pl(12).border_l(2).border_color(t.border).children(m.capabilities.map((c, i) => this.capability(t, m, c, i))));
    }
    return row;
  }

  /** The hosts installed and running; with none, what installing one means, and how. */
  core(t) {
    const line = (text, color = t.dim) => div().text_size(12.5).line_height("18px").text_color(color).child(text);
    const box = v_flex().w_full().py(12).gap(6).border_b(1).border_color(t.border)
      .child(div().text_size(14).text_color(t.foreground).child("Core"));
    const hosts = this.hosts ?? [];
    if (hosts.length) {
      box.child(line(`wasm-mods-host installed: ${hosts.map((h) => `${h.version} (${protocols(h.protocol)})`).join(", ")}`));
      box.child(line(this.running ? `Running for this Herdr: ${this.running.version} (port ${this.running.port})` : "Not running: it starts when a WASM mod is shown."));
    }
    const n = this.offer;
    if (n) {
      const host = n.lock?.host, file = host && n.target ? host.files[n.target] : null;
      box.child(line("No wasm-mods-host is installed: WASM mods can't run until one is.", t.foreground));
      if (host) box.child(line(`Installs ${host.version} into ~/.local/share/wasm-mods/host/${host.version}/${n.url ? `, from ${n.url} (sha256 ${file.sha256.slice(0, 16)}…)` : ""}.`));
      if (n.error) box.child(line(n.error, t.blocked));
      const buttons = h_flex().gap(8);
      if (n.url && !n.error) buttons.child(this.button(t, "core-download", `Download v${host.version}${file.size ? ` (${(file.size / 1048576).toFixed(1)} MB, checksum pinned)` : " (checksum pinned)"}`, (_e, cx) => this.install(cx, "download")));
      if (host?.source && n.canBuild) buttons.child(this.button(t, "core-build", "Build from source", (_e, cx) => this.install(cx, "build")));
      box.child(buttons);
    }
    if (this.installing) box.child(line(this.installing, t.muted));
    return box;
  }

  install(cx, how) {
    if (this.installing || !this.offer) return;
    const n = this.offer;
    this.installing = how === "download" ? "Downloading…" : "Building… (a few minutes)";
    this.error = null;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        if (how === "download") await downloadHost(n.lock, n.target);
        else await buildHost(cx, n.lock, (text) => { this.installing = text; cx.notify(); });
        this.note = "wasm-mods-host installed";
      } catch (e) {
        this.error = String(e?.message ?? e);
      }
      this.installing = null;
      this.load(cx);
    });
  }

  render() {
    const t = theme();
    const body = v_flex().w_full().px(24).pb(8).child(this.core(t));
    if (this.offer) { /* no host: the core section says what to do */ }
    else if (this.modules === null) body.child(div().py(20).text_size(13).text_color(t.dim).child("Reading the modules…"));
    else if (this.modules.length === 0 && !this.error) body.child(div().py(20).text_size(13).text_color(t.dim).child("No WASM modules installed."));
    else body.children(this.modules.map((m) => this.module(t, m)));
    const status = this.error ?? this.note;
    return v_flex().id("wasm-permissions").track_focus(this.handle).w_full().h(600).min_h(0).bg(t.popover)
      .on_key_down((e, cx) => {
        if (e.key !== "escape") return;
        if (this.confirm) { this.confirm = null; cx.notify(); }
        else this.close();
        cx.stop_propagation();
      })
      // The title drags the window; everything below keeps its presses.
      .child(v_flex().flex_none().px(24).pt(18).pb(10).gap(4).cursor_grab()
        .child(div().text_size(19).font_semibold().text_color(t.foreground).child("WASM Mods"))
        .child(div().text_size(12.5).line_height("18px").text_color(t.dim)
          .child("What each module may do. A capability is allowed only from the module's own prompt; here you can deny it or have it ask again.")))
      .child(div().relative().flex_1().min_h(0).w_full().on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .child(div().id("wp-body").size_full().overflow_y_scroll().child(body))
        .child(Scrollbar.vertical("wp-body").absolute().inset_0()))
      .child(h_flex().flex_none().px(24).py(14).gap(10).items_center().border_t(1).border_color(t.border)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .child(div().flex_1().min_w(0).text_size(12.5).text_color(this.error ? t.blocked : t.muted).child(this.busy ? "Changing…" : status ?? ""))
        .child(this.button(t, "wp-close", "Done", () => this.close())));
  }
}
