use std::fs;
use std::path::PathBuf;

fn read_ui_file(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
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
    assert!(components.contains("height: (root.prominent ? 56px : 48px) * VxTheme.s;"));
    assert!(components.contains("font-size: (root.prominent ? 15px : 14px) * VxTheme.s;"));
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

    assert!(space_ui.contains("text <=> root.user-status-input;"));
    assert!(space_ui.contains("text <=> root.user-status-input;\n                                    placeholder: \"Set status\";\n                                    horizontal-stretch: 1;"));
    assert!(space_ui.contains("label: \"Set\";\n                                    glyph: \"ST\";\n                                    soft: true;\n                                    horizontal-stretch: 1;"));

    assert!(space_ui.contains("text <=> root.user-bio-input;"));
    assert!(space_ui.contains("text <=> root.user-bio-input;\n                                    placeholder: \"Set bio\";\n                                    horizontal-stretch: 1;"));
    assert!(space_ui.contains("label: \"Save\";\n                                    glyph: \"SV\";\n                                    soft: true;\n                                    horizontal-stretch: 1;"));

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
    let share_button = snippet(
        &room_ui,
        "root.show-share-config ? \"Close Share\" : \"Share\";",
        420,
    );
    assert!(share_button.contains("enabled: root.is-sharing-screen || !root.has-screen-share;"));
    assert!(!share_button.contains("root.toggle-screen-share();"));
    assert!(room_ui.contains("root.refresh-screen-share-sources();"));

    let share_header = snippet(&room_ui, "label: \"Start\";", 320);
    assert!(share_header.contains("enabled: root.selected-screen-share-source >= 0"));
    assert!(room_ui.contains("clicked => { root.toggle-screen-share(); }"));
}
