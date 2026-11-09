    // The quake window on GNOME Wayland. On Wayland only the shell can
    // place a window, keep it above others and on every workspace, leave
    // it out of Alt+Tab and the taskbar, or see whether another window
    // covers it; this small part of jot lives in the shell and answers
    // over a Unix socket in XDG_RUNTIME_DIR (and D-Bus).

    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this);
        this._dbus.export(Gio.DBus.session, '/io/github/jeffjose/Jot');
        this._listen();
        this._mapped = global.window_manager.connect('map', (_wm, actor) => {
            this._onMapped(actor.meta_window);
        });
        this._focused = global.display.connect('notify::focus-window', () => this._onFocused());
    }

    disable() {
        global.window_manager.disconnect(this._mapped);
        global.display.disconnect(this._focused);
        this._track(null);
        this._want = null;
        this._target = null;
        this._restart = null;
        if (this._socket) {
            try { this._socket.stop(); this._socket.close(); } catch (_) {}
            this._socket = null;
        }
        if (this._socketPath) {
            try { GLib.unlink(this._socketPath); } catch (_) {}
            this._socketPath = null;
        }
        this._dbus.unexport();
        this._dbus = null;
    }

    // A socket in XDG_RUNTIME_DIR so jot can ask for the pointer, the work
    // area, or a new size while dragging without starting a gdbus process.
    _listen() {
        if (!Gio.SocketService || !Gio.UnixSocketAddress)
            return;
        try {
            const dir = GLib.getenv('XDG_RUNTIME_DIR');
            const path = dir
                ? GLib.build_filenamev([dir, 'jot-shell.sock'])
                : `/tmp/jot-${GLib.get_user_name()}-shell.sock`;
            try { GLib.unlink(path); } catch (_) {}
            const service = new Gio.SocketService();
            service.add_address(
                Gio.UnixSocketAddress.new(path),
                Gio.SocketType.STREAM,
                Gio.SocketProtocol.DEFAULT,
                null
            );
            service.connect('incoming', (_service, conn) => {
                const input = new Gio.DataInputStream({base_stream: conn.get_input_stream()});
                input.read_line_async(GLib.PRIORITY_DEFAULT, null, (stream, res) => {
                    try {
                        const [line] = stream.read_line_finish_utf8(res);
                        if (line) {
                            const reply = this.Quake(line) + '\n';
                            conn.get_output_stream().write_all(reply, null);
                        }
                    } catch (_) {
                    } finally {
                        try { conn.close(null); } catch (_) {}
                    }
                });
                return true;
            });
            service.start();
            this._socket = service;
            this._socketPath = path;
        } catch (e) {
            log(`jot: socket listener failed: ${e.message}`);
        }
    }

    _ours(window) {
        if (!window)
            return false;
        return window.get_wm_class() === 'jot-quake' ||
            window.get_gtk_application_id?.() === 'jot-quake' ||
            window.get_sandboxed_app_id?.() === 'jot-quake';
    }

    _window() {
        const windows = global.get_window_actors().map(actor => actor.meta_window);
        return windows.find(window => this._ours(window)) ?? null;
    }

    // Whether a window above `window` in the shell's stacking, on the
    // workspace shown and not minimized, covers a quarter of it or more.
    _covered(window) {
        const list = global.get_window_actors().map(actor => actor.meta_window);
        const at = list.indexOf(window);
        if (at < 0)
            return false;
        const r = window.get_frame_rect();
        if (r.width <= 0 || r.height <= 0)
            return false;
        const desk = global.workspace_manager.get_active_workspace_index();
        for (let i = at + 1; i < list.length; i++) {
            const o = list[i];
            if (!o || o.minimized)
                continue;
            const on = o.is_on_all_workspaces() || (o.get_workspace?.()?.index() ?? desk) === desk;
            if (!on)
                continue;
            const or = o.get_frame_rect();
            const iw = Math.min(r.x + r.width, or.x + or.width) - Math.max(r.x, or.x);
            const ih = Math.min(r.y + r.height, or.y + or.height) - Math.max(r.y, or.y);
            if (iw > 0 && ih > 0 && iw * ih * 4 >= r.width * r.height)
                return true;
        }
        return false;
    }

    Quake(asked) {
        const want = JSON.parse(asked);
        if (want.rect || want.pixels || !this._want) {
            if (want.rect && !want.pixels)
                want.pixels = null;
            this._want = Object.assign(this._want ?? {}, want);
        } else if (want.blur !== undefined) {
            this._want.blur = want.blur;
        }
        const window = this._window();
        if (want.do === 'restart') {
            this._restart = want;
            if (!window) {
                this._restart = null;
                return this._reply(this._start(want) ? 'starting' : 'absent', null);
            }
            this._track(window);
            return this._reply('restarting', window);
        }
        if (!window) {
            if (want.do === 'start' || (want.do === 'toggle' && want.run))
                return this._reply(this._start(want) ? 'starting' : 'absent', null);
            this._pending = want.do === 'toggle' || want.do === 'show';
            this._pendingHidden = false;
            return this._reply('absent', null);
        }
        this._track(window);
        const shown = !window.minimized;
        if (want.do === 'state')
            return this._reply(shown ? 'shown' : 'hidden', window);
        if (want.do === 'place') {
            if (shown)
                this._place(window);
            return this._reply(shown ? 'shown' : 'hidden', window);
        }
        const focused = global.display.focus_window === window;
        const away = want.do === 'hide' ||
            (want.do === 'toggle' && shown && (focused || !this._covered(window)));
        if (away) {
            if (shown)
                this._hide(window);
            return this._reply('hidden', window);
        }
        this._show(window);
        return this._reply('shown', window);
    }

    // Start the quake window as a client of the shell's own, which is what
    // lets the shell leave it out of Alt+Tab, the overview and the dash.
    _start(want) {
        if ((Meta.is_wayland_compositor && !Meta.is_wayland_compositor()) || !want.run)
            return false;
        try {
            const launcher = new Gio.SubprocessLauncher({flags: Gio.SubprocessFlags.NONE});
            if (want.path)
                launcher.setenv('PATH', want.path, true);
            launcher.set_cwd(GLib.get_home_dir());
            let client;
            if (Meta.WaylandClient.new_subprocess) {
                client = Meta.WaylandClient.new_subprocess(global.context, launcher, want.run);
            } else {
                try {
                    client = Meta.WaylandClient.new(global.context, launcher);
                } catch (_) {
                    client = Meta.WaylandClient.new(launcher);
                }
                client.spawnv(global.display, want.run);
            }
            this._client = client;
            this._pending = !want.hidden;
            this._pendingHidden = !!want.hidden;
            return true;
        } catch (e) {
            log(`jot: the shell cannot start the quake window itself: ${e.message}`);
            return false;
        }
    }

    _onMapped(window) {
        if (!window || !this._ours(window))
            return;
        try {
            if (this._client?.owns_window(window))
                this._client.hide_from_window_list(window);
        } catch (e) {
            log(`jot: quake window stays in the window list: ${e.message}`);
        }
        this._track(window);
        if (this._pendingHidden) {
            this._pendingHidden = false;
            this._pending = false;
            this._hide(window);
            return;
        }
        if (!this._pending && !this._want)
            return;
        this._pending = false;
        this._show(window);
    }

    _track(window) {
        if (this._tracked === window)
            return;
        if (this._tracked) {
            for (const id of this._trackedIds)
                this._tracked.disconnect(id);
        }
        this._tracked = window;
        this._trackedIds = [];
        if (!window)
            return;
        // Wayland resizes when the client commits its buffer for the new
        // size; put the frame back on its target if Mutter shifted it.
        const fixup = () => {
            if (this._fixing || window.minimized || !this._target)
                return;
            const r = window.get_frame_rect();
            const t = this._target;
            if (r.x !== t.x || r.y !== t.y || r.width !== t.width || r.height !== t.height) {
                this._fixing = true;
                try {
                    window.move_resize_frame(true, t.x, t.y, t.width, t.height);
                } finally {
                    this._fixing = false;
                }
            }
        };
        this._trackedIds.push(window.connect('size-changed', fixup));
        this._trackedIds.push(window.connect('position-changed', fixup));
        this._trackedIds.push(window.connect('unmanaged', () => {
            this._tracked = null;
            this._trackedIds = [];
            this._target = null;
            if (this._restart) {
                const want = this._restart;
                this._restart = null;
                this._start(want);
            }
        }));
    }

    _onFocused() {
        const focus = global.display.focus_window;
        if (!this._want?.blur || !focus || this._ours(focus))
            return;
        const window = this._window();
        if (window && !window.minimized && focus.get_transient_for?.() !== window)
            this._hide(window);
    }

    _monitor(want) {
        const display = global.display;
        return (want?.pointer ?? true) ? display.get_current_monitor() : display.get_primary_monitor();
    }

    _workArea(window, want) {
        const monitor = this._monitor(want);
        if (window)
            return window.get_work_area_for_monitor(monitor);
        if (Main.layoutManager?.getWorkAreaForMonitor)
            return Main.layoutManager.getWorkAreaForMonitor(monitor);
        return global.display.get_monitor_geometry(monitor);
    }

    _targetFor(window, want) {
        const monitor = this._monitor(want);
        const area = this._workArea(window, want);
        if (want.pixels) {
            const [x, y, width, height] = want.pixels;
            return {monitor, area, x, y, width: Math.max(1, width), height: Math.max(1, height)};
        }
        const [rx, ry, rw, rh] = want.rect ?? [0, 0, 1, 0.4];
        const x = area.x + Math.round(rx * area.width);
        const width = Math.max(1, Math.round(rw * area.width));
        const height = Math.max(1, Math.round(rh * area.height));
        const y = ry > 0 ? area.y + area.height - height : area.y;
        return {monitor, area, x, y, width, height};
    }

    _applyGeometry(window, want) {
        if (window.is_fullscreen())
            window.unmake_fullscreen();
        const maximized = window.get_maximized ? window.get_maximized() !== 0 : window.is_maximized?.();
        if (maximized) {
            try {
                window.unmaximize(Meta.MaximizeFlags.BOTH);
            } catch (_) {
                window.unmaximize();
            }
        }
        const target = this._targetFor(window, want);
        if (window.get_monitor() !== target.monitor)
            window.move_to_monitor(target.monitor);
        this._target = {x: target.x, y: target.y, width: target.width, height: target.height};
        this._fixing = true;
        try {
            window.move_resize_frame(true, target.x, target.y, target.width, target.height);
        } finally {
            this._fixing = false;
        }
        if (!window.is_above())
            window.make_above();
        if (want.all_workspaces ?? true) {
            if (!window.is_on_all_workspaces())
                window.stick();
        } else if (window.is_on_all_workspaces()) {
            window.unstick?.();
        }
    }

    _show(window) {
        const want = this._want ?? {};
        const actor = window.get_compositor_private();
        if (window.minimized) {
            if (actor)
                Main.wm.skipNextEffect(actor);
            window.unminimize();
        }
        this._applyGeometry(window, want);
        Main.activateWindow(window);
    }

    _place(window) {
        if (window.minimized)
            return;
        this._applyGeometry(window, this._want ?? {});
    }

    _hide(window) {
        this._target = null;
        const actor = window.get_compositor_private();
        if (actor)
            Main.wm.skipNextEffect(actor);
        window.minimize();
    }

    _reply(state, window) {
        const want = this._want ?? {};
        const area = this._workArea(window, want);
        const pointer = global.get_pointer ? global.get_pointer() : [0, 0];
        const frame = window ? window.get_frame_rect() : null;
        return JSON.stringify({
            version: VERSION,
            state,
            focused: !!window && global.display.focus_window === window,
            covered: !!window && !window.minimized && this._covered(window),
            pointer: [pointer[0], pointer[1]],
            area: [area.x, area.y, area.width, area.height],
            frame: frame ? [frame.x, frame.y, frame.width, frame.height] : null,
        });
    }
