use std::fs;
use std::path::PathBuf;

fn read_ui_file(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    // Normalize CRLF: Windows checkouts (core.autocrlf) would otherwise break
    // the multi-line literal assertions below.
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        .replace("\r\n", "\n")
}

fn snippet<'a>(content: &'a str, anchor: &str, radius: usize) -> &'a str {
    let index = content
        .find(anchor)
        .unwrap_or_else(|| panic!("missing anchor: {anchor}"));
    let start = index.saturating_sub(radius);
    let end = (index + anchor.len() + radius).min(content.len());
    &content[start..end]
}

#[test]
fn shared_input_supports_prominent_visibility_variant() {
    let components = read_ui_file("ui/components.slint");
    assert!(components.contains("in property <bool> prominent: false;"));
    assert!(components.contains("height: (root.prominent ? 44px : 36px) * VxTheme.s;"));
    assert!(components.contains("font-size: (root.prominent ? 14px : 13px) * VxTheme.s;"));
    // The field is custom-drawn on the TextInput primitive — std LineEdit
    // painted platform chrome that fought the theme and clipped descenders.
    assert!(components.contains("inner := TextInput {"));
}

#[test]
fn critical_text_entry_points_use_prominent_inputs() {
    let main_ui = read_ui_file("ui/main.slint");
    assert!(snippet(&main_ui, "text <=> root.auth-email;", 220).contains("prominent: true;"));
    assert!(snippet(&main_ui, "text <=> root.auth-password;", 220).contains("prominent: true;"));
    assert!(
        snippet(&main_ui, "text <=> root.quick-switcher-query;", 180).contains("prominent: true;")
    );

    let chat_ui = read_ui_file("ui/views/chat_view.slint");
    assert!(chat_ui.contains("composer-edit-stacked-main := TextEdit {"));
    assert!(chat_ui.contains("composer-edit-wide-main := TextEdit {"));
    assert!(chat_ui.contains("composer-surface-wide := Rectangle {"));
    assert!(chat_ui.contains("text <=> root.chat-input;"));
    assert!(snippet(&chat_ui, "text <=> root.chat-search-query;", 180).contains("prominent: true;"));

    let system_ui = read_ui_file("ui/views/system_view.slint");
    assert!(snippet(&system_ui, "add-friend-narrow := VxInput", 260).contains("prominent: true;"));
    assert!(snippet(&system_ui, "add-friend-wide := VxInput", 180).contains("prominent: true;"));
}

#[test]
fn compact_forms_keep_full_width_inputs_and_actions() {
    let home_ui = read_ui_file("ui/views/home_view.slint");
    let quick_call_password = snippet(&home_ui, "text <=> root.room-password;", 360);
    assert!(quick_call_password.contains("prominent: true;"));
    assert!(quick_call_password.contains("label: \"Join\";"));
    assert!(quick_call_password.contains("horizontal-stretch: 1;"));

    let space_ui = read_ui_file("ui/views/space_view.slint");
    assert!(space_ui.contains("text <=> root.new-channel-name;"));
    assert!(space_ui.contains("text <=> root.new-channel-name;\n                                    placeholder: \"channel-name\";\n                                    horizontal-stretch: 1;"));
    assert!(space_ui.contains("label: \"Create\";\n                                    accent: true;\n                                    horizontal-stretch: 1;"));

    // Profile rows pair a stretching input with a hugging action button —
    // full-width action slabs were part of the pre-v0.14 jank.
    assert!(space_ui.contains("text <=> root.user-status-input;"));
    assert!(space_ui.contains("text <=> root.user-status-input;\n                                    placeholder: \"Set status\";\n                                    horizontal-stretch: 1;"));
    assert!(space_ui.contains("label: \"Set\";\n                                    small: true;\n                                    soft: true;"));

    assert!(space_ui.contains("text <=> root.user-bio-input;"));
    assert!(space_ui.contains("text <=> root.user-bio-input;\n                                    placeholder: \"Set bio\";\n                                    horizontal-stretch: 1;"));
    assert!(space_ui.contains("label: \"Save\";\n                                    small: true;\n                                    soft: true;"));

    let settings_ui = read_ui_file("ui/views/settings_view.slint");
    assert!(settings_ui.contains("text <=> root.new-clip-path;"));
    assert!(settings_ui.contains("text <=> root.new-clip-path;\n                            placeholder: \"Path to .wav file\";\n                            horizontal-stretch: 1;"));
    assert!(settings_ui.contains("label: \"Add\";\n                            accent: true;\n                            horizontal-stretch: 1;"));
}

#[test]
fn account_management_forms_stay_stacked_and_readable() {
    let settings_ui = read_ui_file("ui/views/settings_view.slint");
    assert!(snippet(&settings_ui, "text <=> root.acct-new-name;", 220).contains("prominent: true;"));
    assert!(snippet(&settings_ui, "text <=> root.acct-old-pw;", 220).contains("prominent: true;"));
    assert!(snippet(&settings_ui, "text <=> root.acct-new-pw;", 220).contains("prominent: true;"));
    assert!(
        snippet(&settings_ui, "text <=> root.acct-delete-confirm;", 220)
            .contains("prominent: true;")
    );
    assert!(settings_ui.contains(
        "FieldLabel { text: \"Display Name\"; }\n                        VerticalLayout {"
    ));
    assert!(settings_ui.contains(
        "FieldLabel { text: \"Change Password\"; }\n                        VerticalLayout {"
    ));
    assert!(settings_ui.contains("FieldLabel { text: \"Danger Zone\"; }"));
}

#[test]
fn chat_shell_and_composer_adapt_before_the_ui_gets_squeezed() {
    let main_ui = read_ui_file("ui/main.slint");
    // Breakpoints must be applied on the first paint as well as on resize. With
    // only a `changed width` handler they kept their `true` defaults until the
    // window happened to change size, so a narrow window was first laid out as
    // a three-column desktop with the chat transcript squeezed to nothing.
    // (They are assignments rather than bindings on purpose: binding them to
    // `root.width` closes a layout loop Slint may panic on.)
    let init_block = snippet(&main_ui, "    init => {", 200);
    assert!(init_block.contains("root.desktop-layout = root.width >= 960px;"));
    assert!(init_block.contains("root.shell-compact = root.width < 1280px;"));
    assert!(main_ui.contains("root.desktop-layout = self.width >= 960px;"));
    assert!(main_ui.contains("root.shell-compact = self.width < 1280px;"));
    assert!(main_ui.contains("property <bool> chat-focus-layout: root.current-view == 5;"));
    assert!(
        main_ui.contains("property <bool> rail-open: (root.desktop-layout && !root.chat-focus-layout) || sidebar-expanded;")
    );
    assert!(main_ui.contains("show-nav-button: !root.desktop-layout || root.chat-focus-layout;"));
    assert!(main_ui.contains("show-members: root.current-view == 4 && !root.shell-compact;"));
    assert!(main_ui.contains("if !root.chat-focus-layout && root.rail-open : WorkspaceRail {"));
    let chat_mount = snippet(&main_ui, "if root.current-view == 5 : Rectangle {", 3600);
    assert!(chat_mount.contains("width: parent.width;"));
    assert!(chat_mount.contains("height: parent.height;"));
    assert!(main_ui.contains("chat-workspace-sidebar := Rectangle {"));
    assert!(main_ui.contains("chat-workspace-content := Rectangle {"));
    assert!(main_ui.contains("text: \"TEXT CHANNELS\";"));
    assert!(main_ui.contains("clicked => { root.select-text-channel(channel.id); }"));
    assert!(main_ui.contains("clicked => { root.join-channel(channel.id); }"));
    assert!(main_ui.contains("if root.chat-is-direct-message || !root.desktop-layout : ChatView {"));

    let chat_ui = read_ui_file("ui/views/chat_view.slint");
    assert!(chat_ui
        .contains("property <bool> stacked-composer: root.compact-mode || root.width < 760px;"));
    assert!(chat_ui.contains("property <string> composer-placeholder:"));
    assert!(chat_ui.contains("property <string> composer-secondary-hint:"));
    assert!(chat_ui.contains("function submit-composer() {"));
    assert!(chat_ui.contains("function composer-key-handler(event: KeyEvent) -> EventResult {"));
    assert!(chat_ui.contains("composer-shell := Rectangle {"));
    assert!(chat_ui.contains("property <length> composer-input-height:"));
    assert!(chat_ui.contains("composer-edit-wide-main := TextEdit {"));
    assert!(chat_ui.contains("composer-surface-wide := Rectangle {"));
    assert!(chat_ui.contains("width: parent.width;"));
    assert!(chat_ui.contains("height: parent.height;"));
    assert!(chat_ui.contains("min-width: 0px;"));
    assert!(chat_ui.contains("Direct message"));
    assert!(chat_ui.contains("Enter sends · Shift+Enter adds a new line"));
    assert!(chat_ui.contains("border-color: VxTheme.accent-border;"));
}

#[test]
fn screen_share_preview_uses_a_dedicated_popout_window() {
    let main_ui = read_ui_file("ui/main.slint");
    assert!(
        main_ui.contains("export { ScreenShareWidgetWindow } from \"screen_share_widget.slint\";")
    );
    assert!(!main_ui.contains("root.current-view != 1 && root.room-code != \"\" && root.is-connected && root.has-screen-share : ScreenSharePip"));

    let widget_ui = read_ui_file("ui/screen_share_widget.slint");
    assert!(widget_ui.contains("export component ScreenShareWidgetWindow inherits Window {"));
    assert!(widget_ui.contains("always-on-top: true;"));
    assert!(widget_ui.contains("no-frame: false;"));
    assert!(widget_ui.contains("has-screen-image"));
    assert!(widget_ui.contains("callback dismiss();"));
    assert!(widget_ui.contains("callback drag-begin();"));
    assert!(widget_ui.contains("callback drag-move(float, float);"));
    assert!(widget_ui.contains("callback drag-end();"));
    assert!(snippet(&widget_ui, "\"Your screen is live\"", 120).contains("root.is-sharing-screen"));
    assert!(widget_ui.contains("Share stays live while you browse"));
    assert!(widget_ui.contains("clicked => { root.focus-room(); }"));
    assert!(widget_ui.contains("clicked => { root.dismiss(); }"));
}

#[test]
fn screen_share_stays_explicit_until_start() {
    let room_ui = read_ui_file("ui/views/room_view.slint");

    // Anchored on the control's own glyph rather than on a byte window after
    // its label: the previous version passed only because `toggle-screen-share`
    // happened to fall 26 characters outside the window it inspected, so any
    // reordering of the control's properties silently changed what was tested.
    let share_control = snippet(&room_ui, "glyph: \"SH\";", 1200);
    assert!(
        share_control.contains("enabled: root.is-sharing-screen || !root.has-screen-share;"),
        "the share control must stay disabled while somebody else is sharing"
    );
    assert!(
        share_control.contains("root.show-share-config = !root.show-share-config;"),
        "the share control must open the source picker, not begin a share"
    );
    assert!(
        share_control.contains("root.refresh-screen-share-sources();"),
        "opening the picker must refresh the source list"
    );

    // The only path from the control to `toggle-screen-share` is the branch
    // that stops a share already running.
    let after_stop_branch = share_control
        .split("if root.is-sharing-screen {")
        .nth(1)
        .expect("the share control should branch on is-sharing-screen");
    let stop_branch = after_stop_branch
        .split("} else if")
        .next()
        .expect("the is-sharing-screen branch should be followed by an else-if");
    assert!(
        stop_branch.contains("root.toggle-screen-share();"),
        "the is-sharing-screen branch should stop the share"
    );
    let picker_branch = after_stop_branch
        .split("} else if")
        .nth(1)
        .expect("the share control should have an else-if branch");
    assert!(
        !picker_branch.contains("root.toggle-screen-share();"),
        "opening the picker must not start a share"
    );

    // Starting is explicit and gated on having chosen a source.
    let share_header = snippet(&room_ui, "label: \"Start\";", 320);
    assert!(share_header.contains("enabled: root.selected-screen-share-source >= 0"));
    assert!(room_ui.contains("clicked => { root.toggle-screen-share(); }"));
}

#[test]
fn call_controls_are_fixed_size_and_labelled_for_assistive_tech() {
    let components = read_ui_file("ui/components.slint");
    // The in-call controls are icon-only, so the accessible name is the only
    // name they have.
    let call_button = snippet(&components, "export component VxCallButton", 1400);
    assert!(call_button.contains("accessible-role: button;"));
    assert!(call_button.contains("accessible-label: root.label;"));
    // They must never stretch: the call bar used to be full-width text slabs.
    assert!(call_button.contains("horizontal-stretch: 0;"));

    let room_ui = read_ui_file("ui/views/room_view.slint");
    for expected in [
        "label: root.is-muted ? \"Unmute microphone\" : \"Mute microphone\";",
        "label: root.is-deafened ? \"Undeafen\" : \"Deafen\";",
        "label: \"Audio settings\";",
    ] {
        assert!(
            room_ui.contains(expected),
            "call control missing accessible label: {expected}"
        );
    }
    // No control in the room view stretches to fill the bar.
    assert!(
        !room_ui.contains("horizontal-stretch: 1;\n                        clicked =>"),
        "call controls must not stretch"
    );
}

#[test]
fn navigation_labels_do_not_change_with_the_theme_preset() {
    // Presets are accent hues only. The bottom tab bar used to rename itself
    // per preset — "HOME" became "GUIDE" or "BRIDGE", "SYS" became "STATS" —
    // which moved both the visible wording and the accessible name with the
    // colour scheme.
    let shell = read_ui_file("ui/shell.slint");
    let bottom_nav = snippet(&shell, "export component BottomNav", 1600);
    for banned in ["is-party", "is-retro", "is-noir", "is-arctic", "is-amber"] {
        assert!(
            !bottom_nav.contains(banned),
            "bottom navigation must not branch on theme preset: {banned}"
        );
    }
    for label in [
        "label: \"Home\";",
        "label: \"Settings\";",
        "label: \"System\";",
    ] {
        assert!(
            bottom_nav.contains(label),
            "missing stable nav label: {label}"
        );
    }
}

#[test]
fn surfaces_stay_flat_and_shadow_free() {
    // The v0.14 language is tonal surfaces plus 1px hairlines: no gradients,
    // no glows, no drop shadows (the project bans them for GPU cost). These
    // had survived in the shell background, the room header and stage, the
    // theme-preset cards and the pop-out windows.
    for file in [
        "ui/main.slint",
        "ui/shell.slint",
        "ui/components.slint",
        "ui/theme.slint",
        "ui/member_widget.slint",
        "ui/screen_share_widget.slint",
        "ui/views/room_view.slint",
        "ui/views/home_view.slint",
        "ui/views/settings_view.slint",
        "ui/views/system_view.slint",
        "ui/views/space_view.slint",
        "ui/views/chat_view.slint",
    ] {
        let content = read_ui_file(file);
        assert!(
            !content.contains("@linear-gradient") && !content.contains("@radial-gradient"),
            "{file} reintroduces a gradient"
        );
        assert!(
            !content.contains("drop-shadow"),
            "{file} reintroduces a drop shadow"
        );
    }
}
