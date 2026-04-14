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
    assert!(snippet(&chat_ui, "text <=> root.chat-input;", 220).contains("prominent: true;"));
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
