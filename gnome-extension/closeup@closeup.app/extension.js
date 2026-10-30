import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {getIBusManager} from 'resource:///org/gnome/shell/misc/ibusManager.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const PROTOCOL_VERSION = 2;
const OBJECT_PATH = '/app/closeup/Overlay';
const STAGE_TITLE = 'Closeup Stage';
const GAP = 6;
const STROKE = 3;
const MARGIN = 8;
const TYPING_POLL_MS = 100;
const PICKER_WAIT_MS = 60000;
const PORTAL_WINDOW = /xdg-desktop-portal/;

const ACCESS_WARNINGS = {
    stage: 'Global keys off: log out and back in (or focus the Stage window)',
    chords: 'Bare digits off: /dev/uinput is not writable, use Ctrl+Alt+digit',
    full: null,
};

const INTERFACE = `
<node>
  <interface name="app.closeup.Overlay">
    <method name="SetSource">
      <arg type="s" direction="in" name="kind"/>
      <arg type="i" direction="in" name="x"/>
      <arg type="i" direction="in" name="y"/>
      <arg type="i" direction="in" name="width"/>
      <arg type="i" direction="in" name="height"/>
    </method>
    <method name="Show">
      <arg type="d" direction="in" name="x"/>
      <arg type="d" direction="in" name="y"/>
      <arg type="d" direction="in" name="width"/>
      <arg type="d" direction="in" name="height"/>
      <arg type="d" direction="in" name="zoom"/>
      <arg type="u" direction="in" name="level"/>
      <arg type="b" direction="in" name="isLocked"/>
    </method>
    <method name="Hide"/>
    <method name="Version">
      <arg type="u" direction="out" name="version"/>
    </method>
    <method name="GrabShortcuts">
      <arg type="u" direction="in" name="levels"/>
      <arg type="b" direction="out" name="isGrabbed"/>
    </method>
    <signal name="ShortcutActivated">
      <arg type="s" name="shortcut"/>
    </signal>
    <method name="ExpectPicker">
      <arg type="b" direction="in" name="isExpected"/>
    </method>
    <method name="IsTyping">
      <arg type="b" direction="out" name="isTyping"/>
    </method>
    <method name="SetControls">
      <arg type="b" direction="in" name="isPaused"/>
      <arg type="s" direction="in" name="access"/>
      <arg type="u" direction="in" name="levels"/>
    </method>
  </interface>
</node>`;

function clamp(value, min, max) {
    return Math.min(Math.max(value, min), Math.max(min, max));
}

// A text field has keyboard focus when the shell's input method is focused
// (native Wayland apps, via text-input) or IBus reports a focused context of
// its own (XWayland apps). Either way, bare digits should type.
class TypingTracker {
    constructor(onChange) {
        this._onChange = onChange;
        this._isLegacyFocused = false;
        this._isTyping = this.isTyping;
        this._ibus = getIBusManager();
        this._signals = [
            this._ibus.connect('focus-in', () => this._setLegacy(true)),
            this._ibus.connect('focus-out', () => this._setLegacy(false)),
        ];
        this._poll = GLib.timeout_add(GLib.PRIORITY_DEFAULT, TYPING_POLL_MS, () => {
            this._sync();
            return GLib.SOURCE_CONTINUE;
        });
    }

    get isTyping() {
        return (Main.inputMethod?.currentFocus ?? null) !== null || this._isLegacyFocused;
    }

    _setLegacy(isFocused) {
        this._isLegacyFocused = isFocused;
        this._sync();
    }

    _sync() {
        const isTyping = this.isTyping;
        if (isTyping === this._isTyping)
            return;
        this._isTyping = isTyping;
        this._onChange();
    }

    destroy() {
        this._signals.forEach(id => this._ibus.disconnect(id));
        GLib.source_remove(this._poll);
    }
}

// GNOME's focus-stealing prevention keeps a portal dialog opened by a
// background app behind the active window. While Closeup waits for a
// source choice, bring the picker to the front.
// The shell owns Closeup's Ctrl+Alt shortcuts while the app runs, so they
// work without keyboard access and never leak into the focused app.
class ShortcutGrabber {
    constructor(onActivated) {
        this._actions = new Map();
        this._signal = global.display.connect('accelerator-activated', (display, action) => {
            const name = this._actions.get(action);
            if (name)
                onActivated(name);
        });
    }

    grab(levels) {
        this.release();
        const keys = [['pause', 'p'], ['source', 's'], ['lock', '0']];
        for (let level = 1; level <= Math.min(levels, 9); level++)
            keys.push([`level-${level}`, `${level}`]);
        for (const [name, key] of keys) {
            const action = global.display.grab_accelerator(`<Control><Alt>${key}`, Meta.KeyBindingFlags.NONE);
            if (!action)
                continue;
            Main.wm.allowKeybinding(Meta.external_binding_name_for_action(action),
                Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP);
            this._actions.set(action, name);
        }
        // All or nothing: with a partial set, the app's keyboard service
        // keeps every shortcut, and the grabbed ones would fire twice.
        if (this._actions.size !== keys.length) {
            this.release();
            return false;
        }
        return true;
    }

    release() {
        for (const action of this._actions.keys())
            global.display.ungrab_accelerator(action);
        this._actions.clear();
    }

    destroy() {
        this.release();
        global.display.disconnect(this._signal);
    }
}

class PickerRaiser {
    constructor() {
        this._deadline = 0;
        this._created = global.display.connect('window-created', (display, window) => this._consider(window));
    }

    expect(isExpected) {
        this._deadline = isExpected ? GLib.get_monotonic_time() + PICKER_WAIT_MS * 1000 : 0;
    }

    _consider(window) {
        if (GLib.get_monotonic_time() > this._deadline)
            return;
        const shown = window.connect('shown', () => {
            window.disconnect(shown);
            const id = `${window.get_wm_class() ?? ''} ${window.get_gtk_application_id() ?? ''}`;
            if (PORTAL_WINDOW.test(id))
                Main.activateWindow(window);
        });
    }

    destroy() {
        global.display.disconnect(this._created);
    }
}

class Outline {
    constructor() {
        this.actor = new St.Widget({reactive: false, visible: false});
        this._edges = [0, 1, 2, 3].map(() => {
            const edge = new St.Widget({style_class: 'closeup-edge'});
            this.actor.add_child(edge);
            return edge;
        });
        Main.layoutManager.uiGroup.add_child(this.actor);
    }

    // Drawn outside the shared region so a monitor capture never shows it
    // inside the zoomed output.
    place(rect) {
        const inset = GAP + STROKE;
        const x = rect.x - inset;
        const y = rect.y - inset;
        const width = rect.width + inset * 2;
        const height = rect.height + inset * 2;
        const [top, bottom, left, right] = this._edges;
        top.set_position(x, y);
        top.set_size(width, STROKE);
        bottom.set_position(x, y + height - STROKE);
        bottom.set_size(width, STROKE);
        left.set_position(x, y);
        left.set_size(STROKE, height);
        right.set_position(x + width - STROKE, y);
        right.set_size(STROKE, height);
        Main.layoutManager.uiGroup.set_child_above_sibling(this.actor, null);
        this.actor.show();
    }

    hide() {
        this.actor.hide();
    }

    destroy() {
        this.actor.destroy();
    }
}

class ControlStrip {
    constructor() {
        this.actor = new St.BoxLayout({style_class: 'closeup-strip', reactive: false, visible: false});
        this._zoom = this._add('closeup-zoom');
        this._mode = this._add('closeup-chip');
        this._prefix = this._add('closeup-hint');
        this._levels = new St.BoxLayout({style_class: 'closeup-levels'});
        this.actor.add_child(this._levels);
        this._lock = this._add('closeup-chip');
        this._source = this._add('closeup-chip');
        this._warning = this._add('closeup-warning');
        this._levelChips = [];
        Main.layoutManager.uiGroup.add_child(this.actor);
    }

    _add(styleClass) {
        const label = new St.Label({style_class: styleClass});
        this.actor.add_child(label);
        return label;
    }

    update(controls, isTyping, zoom, level, isLocked) {
        const isBare = controls.access === 'full' && !controls.isPaused && !isTyping;
        const keyPrefix = isBare ? '' : 'Ctrl+Alt+';
        this._zoom.text = `${zoom.toFixed(1)}x`;
        this._mode.text = modeLabel(controls, isTyping);
        setActive(this._mode, isBare);
        this._prefix.text = keyPrefix;
        this._prefix.visible = keyPrefix !== '';
        this._syncLevels(controls.levels, level);
        this._lock.text = `${keyPrefix}0  ${isLocked ? 'locked' : 'lock'}`;
        setActive(this._lock, isLocked);
        this._source.text = 'Ctrl+Alt+S  source';
        const warning = ACCESS_WARNINGS[controls.access] ?? null;
        this._warning.text = warning ?? '';
        this._warning.visible = warning !== null;
    }

    _syncLevels(count, level) {
        while (this._levelChips.length < count) {
            const chip = new St.Label({style_class: 'closeup-chip closeup-level'});
            chip.text = `${this._levelChips.length + 1}`;
            this._levels.add_child(chip);
            this._levelChips.push(chip);
        }
        while (this._levelChips.length > count)
            this._levelChips.pop().destroy();
        this._levelChips.forEach((chip, index) => setActive(chip, index + 1 === level));
    }

    // Prefers the band outside the shared region; window captures never
    // include shell drawings, so a window source can also take it inside.
    place(rect, bounds, isZoomed, isWindow) {
        const [, width] = this.actor.get_preferred_width(-1);
        const [, height] = this.actor.get_preferred_height(width);
        const inset = GAP + STROKE + MARGIN;
        const x = clamp(rect.x + (rect.width - width) / 2, bounds.x + MARGIN,
            bounds.x + bounds.width - width - MARGIN);
        const below = rect.y + rect.height + inset;
        const above = rect.y - inset - height;
        let y = null;
        if (isZoomed && below + height <= bounds.y + bounds.height)
            y = below;
        else if (isZoomed && above >= bounds.y)
            y = above;
        else if (isWindow)
            y = rect.y + rect.height - height - MARGIN * 3;
        if (y === null) {
            this.actor.hide();
            return;
        }
        this.actor.set_position(Math.round(x), Math.round(y));
        Main.layoutManager.uiGroup.set_child_above_sibling(this.actor, null);
        this.actor.show();
    }

    hide() {
        this.actor.hide();
    }

    destroy() {
        this.actor.destroy();
    }
}

function modeLabel(controls, isTyping) {
    if (controls.isPaused)
        return 'PAUSED  Ctrl+Alt+P resumes';
    if (controls.access !== 'full')
        return 'zoom keys';
    return isTyping ? 'typing: digits type' : 'digits zoom';
}

class PanelIndicator {
    constructor() {
        this._button = new PanelMenu.Button(0.0, 'Closeup', false);
        this._label = new St.Label({style_class: 'closeup-panel', y_align: Clutter.ActorAlign.CENTER});
        this._button.add_child(this._label);
        for (const line of [
            '1-9   zoom level (types normally in text fields)',
            'Ctrl+Alt+1-9   zoom level, even while typing',
            'Ctrl+Alt+P   pause or resume bare-digit zoom',
            '0   lock or unlock panning',
            'Ctrl+Alt+S   pick another source',
        ])
            this._button.menu.addMenuItem(new PopupMenu.PopupMenuItem(line, {reactive: false}));
        Main.panel.addToStatusArea('closeup-indicator', this._button);
    }

    update(controls, isTyping, zoom) {
        const state = controls.isPaused ? '  paused' : isTyping ? '  typing' : '';
        this._label.text = `Closeup ${zoom.toFixed(1)}x${state}`;
        setActive(this._label, !controls.isPaused && !isTyping);
    }

    destroy() {
        this._button.destroy();
    }
}

function setActive(actor, isActive) {
    if (isActive)
        actor.add_style_class_name('closeup-active');
    else
        actor.remove_style_class_name('closeup-active');
}

class OverlayService {
    constructor() {
        this._kind = 'monitor';
        this._monitor = global.display.get_primary_monitor();
        this._window = null;
        this._windowSignal = 0;
        this._watch = 0;
        this._sender = null;
        this._controls = {isPaused: false, access: 'full', levels: 9};
        this._view = {x: 0, y: 0, width: 1, height: 1, zoom: 1, level: 1, isLocked: false};
        this._isShown = false;
        this._indicator = null;
        this._outline = new Outline();
        this._strip = new ControlStrip();
        this._typing = new TypingTracker(() => this._render());
        this._picker = new PickerRaiser();
        this._shortcuts = new ShortcutGrabber(name => this.onShortcut?.(name));
        this.onShortcut = null;
    }

    Version() {
        return PROTOCOL_VERSION;
    }

    GrabShortcutsAsync([levels], invocation) {
        this._watchClient(invocation.get_sender());
        invocation.return_value(new GLib.Variant('(b)', [this._shortcuts.grab(levels)]));
    }

    ExpectPicker(isExpected) {
        this._picker.expect(isExpected);
    }

    IsTyping() {
        return this._typing.isTyping;
    }

    SetSource(kind, x, y, width, height) {
        this._kind = kind;
        this._forgetWindow();
        if (kind === 'window')
            this._trackWindow(findWindow(width, height));
        else
            this._monitor = findMonitor(x, y, width, height);
        this._render();
    }

    Show(x, y, width, height, zoom, level, isLocked) {
        this._view = {x, y, width, height, zoom, level, isLocked};
        this._isShown = true;
        this._render();
    }

    Hide() {
        this._isShown = false;
        this._render();
    }

    SetControlsAsync([isPaused, access, levels], invocation) {
        this._watchClient(invocation.get_sender());
        this._controls = {isPaused, access, levels};
        this._indicator ??= new PanelIndicator();
        this._render();
        invocation.return_value(null);
    }

    destroy() {
        this._reset();
        this._shortcuts.destroy();
        this._picker.destroy();
        this._typing.destroy();
        this._outline.destroy();
        this._strip.destroy();
    }

    _render() {
        const view = this._view;
        const isTyping = this._typing.isTyping;
        this._indicator?.update(this._controls, isTyping, view.zoom);
        const bounds = this._isShown ? this._bounds() : null;
        if (!bounds) {
            this._outline.hide();
            this._strip.hide();
            return;
        }
        const rect = {
            x: bounds.x + view.x * bounds.width,
            y: bounds.y + view.y * bounds.height,
            width: view.width * bounds.width,
            height: view.height * bounds.height,
        };
        const isZoomed = view.zoom > 1.001;
        if (isZoomed)
            this._outline.place(rect);
        else
            this._outline.hide();
        this._strip.update(this._controls, isTyping, view.zoom, view.level, view.isLocked);
        this._strip.place(rect, bounds, isZoomed, this._kind === 'window');
    }

    // Clears everything when the app exits or crashes, so no stale outline
    // or indicator is left behind.
    _watchClient(sender) {
        if (sender === this._sender)
            return;
        if (this._watch)
            Gio.bus_unwatch_name(this._watch);
        this._sender = sender;
        this._watch = Gio.bus_watch_name_on_connection(Gio.DBus.session, sender,
            Gio.BusNameWatcherFlags.NONE, null, () => this._reset());
    }

    _reset() {
        if (this._watch)
            Gio.bus_unwatch_name(this._watch);
        this._watch = 0;
        this._sender = null;
        this._isShown = false;
        this._shortcuts.release();
        this._forgetWindow();
        this._indicator?.destroy();
        this._indicator = null;
        this._outline.hide();
        this._strip.hide();
    }

    _bounds() {
        if (this._kind !== 'window')
            return global.display.get_monitor_geometry(this._monitor);
        if (!this._window || this._window.minimized)
            return null;
        return this._window.get_frame_rect();
    }

    _trackWindow(window) {
        this._window = window;
        if (window)
            this._windowSignal = window.connect('unmanaged', () => this._forgetWindow());
    }

    _forgetWindow() {
        if (this._window && this._windowSignal)
            this._window.disconnect(this._windowSignal);
        this._window = null;
        this._windowSignal = 0;
    }
}

function distance(rect, x, y, width, height, scale) {
    return Math.abs(rect.x - x * scale) + Math.abs(rect.y - y * scale) +
        Math.abs(rect.width - width * scale) + Math.abs(rect.height - height * scale);
}

function findMonitor(x, y, width, height) {
    const display = global.display;
    if (width <= 0 || height <= 0)
        return display.get_primary_monitor();
    let best = display.get_primary_monitor();
    let bestScore = Infinity;
    for (let index = 0; index < display.get_n_monitors(); index++) {
        const geometry = display.get_monitor_geometry(index);
        for (const scale of [1, display.get_monitor_scale(index)]) {
            const score = distance(geometry, Math.max(x, 0), Math.max(y, 0), width, height, scale);
            if (score < bestScore) {
                best = index;
                bestScore = score;
            }
        }
    }
    return best;
}

// The portal does not say which window was picked, so take the visible
// window whose size matches the stream, preferring the most recently used.
function findWindow(width, height) {
    const display = global.display;
    const windows = global.get_window_actors()
        .map(actor => actor.get_meta_window())
        .filter(window => window.get_window_type() === Meta.WindowType.NORMAL &&
            !window.minimized && window.get_title() !== STAGE_TITLE);
    const score = window => {
        if (width <= 0 || height <= 0)
            return 0;
        const frame = window.get_frame_rect();
        const scale = display.get_monitor_scale(window.get_monitor());
        return Math.min(...[1, scale, 1 / scale].map(factor =>
            Math.abs(frame.width - width * factor) + Math.abs(frame.height - height * factor)));
    };
    windows.sort((first, second) =>
        score(first) - score(second) || second.get_user_time() - first.get_user_time());
    return windows[0] ?? null;
}

export default class CloseupExtension extends Extension {
    enable() {
        this._service = new OverlayService();
        this._exported = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this._service);
        this._service.onShortcut = name =>
            this._exported?.emit_signal('ShortcutActivated', new GLib.Variant('(s)', [name]));
        this._exported.export(Gio.DBus.session, OBJECT_PATH);
    }

    disable() {
        this._exported?.unexport();
        this._exported = null;
        this._service?.destroy();
        this._service = null;
    }
}
