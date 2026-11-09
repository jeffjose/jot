// A made-up GNOME shell, just enough of one to run jot's extension
// (the variant from before modules, which reads imports.gi) and drive
// the quake window through it. `cargo test` runs it, where there is node.
'use strict';
const fs = require('fs');

const source = fs.readFileSync(process.argv[2], 'utf8');
let failures = 0;
const check = (what, ok) => {
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
    if (!ok) failures++;
};

const GLib = {
    PRIORITY_DEFAULT: 0,
    getenv: () => '/tmp',
    get_user_name: () => 'u',
    get_home_dir: () => '/home/u',
    build_filenamev: parts => parts.join('/'),
    unlink: () => 0,
};

let client = null;
class SubprocessLauncher {
    constructor() { this.env = {}; }
    setenv(name, value) { this.env[name] = value; }
    set_cwd(dir) { this.cwd = dir; }
}
const Gio = {
    DBusExportedObject: {wrapJSObject: () => ({export() {}, unexport() {}})},
    DBus: {session: {}},
    SubprocessLauncher,
    SubprocessFlags: {NONE: 0},
};

let nextId = 1;
class Signals {
    constructor() { this.handlers = new Map(); this.ids = 0; }
    connect(name, f) { const id = ++this.ids; this.handlers.set(id, [name, f]); return id; }
    disconnect(id) { this.handlers.delete(id); }
    emit(name, ...args) {
        for (const [n, f] of [...this.handlers.values()])
            if (n === name)
                f(this, ...args);
    }
}
class Window extends Signals {
    constructor(klass) {
        super();
        this.klass = klass;
        this.id = nextId++;
        this.minimized = false;
        this.frame = {x: 100, y: 100, width: 800, height: 600};
        this.actor = {};
        this.above = false;
        this.sticky = false;
        this.monitor = 0;
        this.workspace = 0;
        this.transientFor = null;
    }
    get_wm_class() { return this.klass; }
    get_id() { return this.id; }
    minimize() {
        this.minimized = true;
        if (display.focus_window === this)
            display.setFocus(null);
    }
    unminimize() { this.minimized = false; }
    is_fullscreen() { return false; }
    get_maximized() { return 0; }
    get_monitor() { return this.monitor; }
    move_to_monitor(m) { this.monitor = m; }
    get_work_area_for_monitor(m) {
        return m === 0
            ? {x: 0, y: 32, width: 1920, height: 1048}
            : {x: 1920, y: 0, width: 2560, height: 1440};
    }
    move_resize_frame(_user, x, y, width, height) { this.frame = {x, y, width, height}; }
    get_frame_rect() { return this.frame; }
    is_above() { return this.above; }
    make_above() { this.above = true; }
    is_on_all_workspaces() { return this.sticky; }
    stick() { this.sticky = true; }
    unstick() { this.sticky = false; }
    get_compositor_private() { return this.actor; }
    get_transient_for() { return this.transientFor; }
    get_workspace() { return {index: () => this.workspace}; }
}
const windows = [];

class Display extends Signals {
    constructor() { super(); this.focus_window = null; this.current = 0; }
    setFocus(window) { this.focus_window = window; this.emit('notify::focus-window'); }
    get_primary_monitor() { return 0; }
    get_current_monitor() { return this.current; }
    get_monitor_geometry(m) {
        return m === 0
            ? {x: 0, y: 32, width: 1920, height: 1048}
            : {x: 1920, y: 0, width: 2560, height: 1440};
    }
}
const display = new Display();
const windowManager = new Signals();
let pointer = [400, 300];

globalThis.global = {
    display,
    window_manager: windowManager,
    context: {},
    get_window_actors: () => windows.map(window => ({meta_window: window})),
    get_pointer: () => pointer,
    workspace_manager: {get_active_workspace_index: () => 0},
};
const Meta = {
    is_wayland_compositor: () => true,
    WaylandClient: {
        new_subprocess: (_context, launcher, argv) => {
            client = {
                launcher,
                argv,
                owned: [],
                hidden: [],
                owns_window: w => client.owned.includes(w),
                hide_from_window_list: w => client.hidden.push(w),
            };
            return client;
        },
    },
    MaximizeFlags: {BOTH: 3},
};
globalThis.imports = {
    gi: {Gio, GLib, Meta},
    ui: {main: {activateWindow: w => display.setFocus(w), wm: {skipNextEffect() {}}, layoutManager: {getWorkAreaForMonitor: m => display.get_monitor_geometry(m)}}},
};
globalThis.log = () => {};

const load = () => new Function(`${source}\nreturn init();`)();
const ext = load();
ext.enable();

const ask = (what, extra = {}) => JSON.parse(ext.Quake(JSON.stringify(Object.assign({
    do: what,
    rect: [0, 0, 1, 0.4],
    pointer: true,
    all_workspaces: true,
    blur: false,
}, extra))));

// 1. Nothing yet: absent, then started by the shell.
check('no quake window: absent', ask('toggle').state === 'absent');
check('the shell starts it', ask('start', {run: ['/usr/bin/jot', '--quake-window'], path: '/home/u/bin:/usr/bin'}).state === 'starting');
check('  with its command and PATH', client && client.argv[1] === '--quake-window' && client.launcher.env.PATH === '/home/u/bin:/usr/bin');

// 2. It maps: hidden from the window list, placed at the top 40% of the work area.
const quake = new Window('jot-quake');
windows.push(quake);
client.owned.push(quake);
windowManager.emit('map', {meta_window: quake});
check('left out of the window list', client.hidden.includes(quake));
check('put at the top, 40% of the work area', JSON.stringify(quake.frame) === JSON.stringify({x: 0, y: 32, width: 1920, height: 419}));
check('above and on every workspace, with the keyboard', quake.above && quake.sticky && display.focus_window === quake);

// 3. Wayland async buffer commit shifting the frame puts it back on target.
quake.frame = {x: 50, y: 50, width: 1200, height: 400};
quake.emit('size-changed');
check('size-changed keeps target frame', JSON.stringify(quake.frame) === JSON.stringify({x: 0, y: 32, width: 1920, height: 419}));

// 4. Toggled while focused: hidden. Toggled again: shown.
check('toggle while focused: hidden', ask('toggle').state === 'hidden' && quake.minimized);
check('toggle while hidden: shown', ask('toggle').state === 'shown' && !quake.minimized && display.focus_window === quake);

// 5. Unfocused but in sight (not covered by >= 25%): toggle hides it on first press.
const small = new Window('notes');
small.frame = {x: 0, y: 600, width: 400, height: 300};
windows.push(small);
display.setFocus(small);
check('toggle when unfocused but uncovered: hidden', ask('toggle').state === 'hidden' && quake.minimized);
ask('show');

// 6. Unfocused and covered by >= 25% by a window above it: toggle brings it forward instead of hiding.
const over = new Window('browser');
over.frame = {x: 0, y: 32, width: 1920, height: 900};
windows.push(over);
display.setFocus(over);
check('covered reported in state', ask('state').covered === true);
check('toggle when covered: brought forward', ask('toggle').state === 'shown' && !quake.minimized && display.focus_window === quake);
windows.pop();

// 7. Live placement by normalized rect and by exact pixels (grip/side drag).
pointer = [960, 540];
const placed = ask('place', {rect: [0.1, 0.5, 0.8, 0.5], all_workspaces: false});
check('place updates rect and unsticks', placed.state === 'shown' && !quake.sticky && JSON.stringify(quake.frame) === JSON.stringify({x: 192, y: 556, width: 1536, height: 524}));
check('place returns pointer and work area', JSON.stringify(placed.pointer) === '[960,540]' && JSON.stringify(placed.area) === '[0,32,1920,1048]');
const pxPlaced = ask('place', {pixels: [120, 32, 1600, 500], rect: null});
check('place with exact pixels', JSON.stringify(pxPlaced.frame) === '[120,32,1600,500]');

// 8. Hide on blur: hides when another window takes focus, ignores transient children.
ask('show', {blur: true});
const child = new Window('dialog');
child.transientFor = quake;
display.setFocus(child);
check('transient child focus does not hide', !quake.minimized);
display.setFocus(small);
check('focus on another window hides when blur is true', quake.minimized);
ask('show', {blur: false});
display.setFocus(small);
check('focus on another window keeps it when blur is false', !quake.minimized);

// 9. Second monitor placement when pointer is on monitor 1.
display.current = 1;
ask('show', {rect: [0, 0, 1, 0.4], pointer: true});
check('moves to pointer monitor', quake.monitor === 1 && JSON.stringify(quake.frame) === JSON.stringify({x: 1920, y: 0, width: 2560, height: 576}));
ask('show', {rect: [0, 0, 1, 0.4], pointer: false});
check('moves to primary monitor when pointer is false', quake.monitor === 0 && quake.frame.x === 0);

// 10. Restart: queues a fresh WaylandClient start when the old window closes.
const restarting = ask('restart', {run: ['/usr/bin/jot', '--quake-window', '--quake-hidden'], hidden: true, path: '/usr/bin'});
check('restart queues for unmanaged', restarting.state === 'restarting');
windows.splice(windows.indexOf(quake), 1);
quake.emit('unmanaged');
check('unmanaged starts new WaylandClient', client.argv.includes('--quake-hidden'));
const quake2 = new Window('jot-quake');
windows.push(quake2);
client.owned.push(quake2);
windowManager.emit('map', {meta_window: quake2});
check('restarted window is hidden from window list and stays minimized', client.hidden.includes(quake2) && quake2.minimized);

ext.disable();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
