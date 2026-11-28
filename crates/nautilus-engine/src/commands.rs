//! An extension's keyboard commands, read from its manifest: the
//! `commands` key, each with the shortcut it suggests for this platform
//! as an osmena chord. Plain data in and out, so it is tested without
//! Chromium. The shell matches the chords (after its own keymap);
//! `chrome.commands.getAll` reports them back in Chrome's notation.

// The callers are the extension manager's native half and the bridge.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use keyboard_types::{Code, Modifiers};
use osmena_core::{ExtensionCommand, KeyChord};
use serde_json::{Value, json};

use crate::extensions::{CommandFacts, KeyPlatform};

/// The commands that stand for a click on the toolbar action. Chrome
/// fires no `onCommand` for them.
pub(crate) fn is_action_command(name: &str) -> bool {
    matches!(
        name,
        "_execute_action" | "_execute_browser_action" | "_execute_page_action"
    )
}

/// The commands a manifest declares, from the manifest's facts
/// (`ManifestFacts::commands`: sorted by name, the order Chrome's
/// `getAll` gives, each with this platform's `suggested_key` as
/// written), each with its description as the manifest has it (`""`
/// when it has none or names a message that is missing) and the chord
/// its key gives on `platform`. `messages` is the parsed
/// `_locales/<default_locale>/messages.json`, when the extension has
/// one. Two commands suggesting one chord: the first by name keeps it.
pub(crate) fn commands_from(
    declared: &[CommandFacts],
    messages: Option<&Value>,
    platform: KeyPlatform,
) -> Vec<ExtensionCommand> {
    let mut taken: Vec<KeyChord> = Vec::new();
    declared
        .iter()
        .map(|fact| {
            let chord = fact
                .suggested_key
                .as_deref()
                .and_then(|key| parse_shortcut(key, platform, !is_action_command(&fact.name)))
                .filter(|chord| !taken.contains(chord));
            taken.extend(chord);
            ExtensionCommand {
                name: fact.name.clone(),
                description: localized(&fact.description, messages),
                chord,
            }
        })
        .collect()
}

/// `text` itself, or the message a `__MSG_name__` key names (matched
/// without regard to case, as Chrome does). A key with no message is
/// `""`.
pub(crate) fn localized(text: &str, messages: Option<&Value>) -> String {
    let Some(name) = text
        .strip_prefix("__MSG_")
        .and_then(|rest| rest.strip_suffix("__"))
    else {
        return text.to_string();
    };
    messages
        .and_then(Value::as_object)
        .and_then(|all| all.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)))
        .and_then(|(_, entry)| entry.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

const LETTERS: [Code; 26] = [
    Code::KeyA,
    Code::KeyB,
    Code::KeyC,
    Code::KeyD,
    Code::KeyE,
    Code::KeyF,
    Code::KeyG,
    Code::KeyH,
    Code::KeyI,
    Code::KeyJ,
    Code::KeyK,
    Code::KeyL,
    Code::KeyM,
    Code::KeyN,
    Code::KeyO,
    Code::KeyP,
    Code::KeyQ,
    Code::KeyR,
    Code::KeyS,
    Code::KeyT,
    Code::KeyU,
    Code::KeyV,
    Code::KeyW,
    Code::KeyX,
    Code::KeyY,
    Code::KeyZ,
];

const DIGITS: [Code; 10] = [
    Code::Digit0,
    Code::Digit1,
    Code::Digit2,
    Code::Digit3,
    Code::Digit4,
    Code::Digit5,
    Code::Digit6,
    Code::Digit7,
    Code::Digit8,
    Code::Digit9,
];

/// The manifest's names for the keys that are neither a letter nor a
/// digit.
const NAMED: [(&str, Code); 18] = [
    ("Comma", Code::Comma),
    ("Period", Code::Period),
    ("Home", Code::Home),
    ("End", Code::End),
    ("PageUp", Code::PageUp),
    ("PageDown", Code::PageDown),
    ("Space", Code::Space),
    ("Insert", Code::Insert),
    ("Delete", Code::Delete),
    ("Up", Code::ArrowUp),
    ("Down", Code::ArrowDown),
    ("Left", Code::ArrowLeft),
    ("Right", Code::ArrowRight),
    ("Tab", Code::Tab),
    ("MediaNextTrack", Code::MediaTrackNext),
    ("MediaPrevTrack", Code::MediaTrackPrevious),
    ("MediaPlayPause", Code::MediaPlayPause),
    ("MediaStop", Code::MediaStop),
];

fn is_media(code: Code) -> bool {
    matches!(
        code,
        Code::MediaTrackNext | Code::MediaTrackPrevious | Code::MediaPlayPause | Code::MediaStop
    )
}

/// The physical key a manifest key name stands for. Case matters, as
/// in Chrome.
fn key_code(token: &str) -> Option<Code> {
    let bytes = token.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0];
        return match c {
            b'A'..=b'Z' => Some(LETTERS[usize::from(c - b'A')]),
            b'0'..=b'9' => Some(DIGITS[usize::from(c - b'0')]),
            _ => None,
        };
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == token)
        .map(|(_, code)| *code)
}

/// The manifest's name for a key [`key_code`] gave.
fn key_name(code: Code) -> String {
    if let Some(i) = LETTERS.iter().position(|c| *c == code) {
        return char::from(b'A' + i as u8).to_string();
    }
    if let Some(i) = DIGITS.iter().position(|c| *c == code) {
        return char::from(b'0' + i as u8).to_string();
    }
    NAMED
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(name, _)| (*name).to_string())
        .unwrap_or_else(|| code.to_string())
}

/// The most tokens a shortcut has: two modifiers and a key
/// (`ui::Command::ParseImpl`: `kMaxTokenSize` less one, since Chrome's
/// manifest parser does not allow Ctrl+Alt).
const MAX_TOKENS: usize = 3;

/// A manifest shortcut (`Ctrl+Shift+Y`, `Command+E`, `MediaStop`) as a
/// chord on `platform`, or `None` for text Chrome would not accept as
/// one. The rules are Chromium's own parser's
/// (`ui/base/accelerators/command.cc`, `ParseImpl`, as
/// `extensions/common/command.cc` calls it; tag 156.0.8078.100): the
/// tokens are split at `+` and trimmed of white space; there are three
/// at most; a modifier may be named twice; there is exactly one key (a
/// letter A to Z, a digit, or a named key, in that case); every key but
/// a media key needs Ctrl, Alt or Command, and Shift alone is none; Alt
/// with Ctrl or with Command is refused (AltGr, and Option with Command
/// on macOS); a media key takes no modifier, and is a shortcut only
/// with `media_keys`, which is false for the action commands. On macOS
/// `Ctrl` is Command (in the `mac` entry too), `MacCtrl` is Control and
/// `Option` is Alt; `Command`, `MacCtrl` and `Option` mean nothing
/// elsewhere.
pub(crate) fn parse_shortcut(
    text: &str,
    platform: KeyPlatform,
    media_keys: bool,
) -> Option<KeyChord> {
    let mac = platform == KeyPlatform::Mac;
    // `base::TRIM_WHITESPACE` for a narrow string: the six ASCII ones.
    let blank = |c: char| matches!(c, ' ' | '\t'..='\r');
    let tokens: Vec<&str> = text
        .split('+')
        .map(|token| token.trim_matches(blank))
        .collect();
    if tokens.len() > MAX_TOKENS {
        return None;
    }
    let mut modifiers = Modifiers::empty();
    let mut code = None;
    for token in tokens {
        let modifier = match token {
            "Ctrl" if mac => Some(Modifiers::META),
            "Ctrl" => Some(Modifiers::CONTROL),
            "Command" if mac => Some(Modifiers::META),
            "MacCtrl" if mac => Some(Modifiers::CONTROL),
            "Alt" => Some(Modifiers::ALT),
            "Option" if mac => Some(Modifiers::ALT),
            "Shift" => Some(Modifiers::SHIFT),
            _ => None,
        };
        match modifier {
            Some(modifier) => modifiers |= modifier,
            None => {
                if code.is_some() {
                    return None;
                }
                code = Some(key_code(token)?);
            }
        }
    }
    let code = code?;
    if is_media(code) {
        return (media_keys && modifiers.is_empty()).then(|| KeyChord::new(modifiers, code));
    }
    let leading = Modifiers::CONTROL | Modifiers::ALT | Modifiers::META;
    if !modifiers.intersects(leading) {
        return None;
    }
    // AltGr on Windows keyboards is Ctrl+Alt; Chrome refuses Alt with
    // Command the same way.
    if modifiers.contains(Modifiers::ALT)
        && modifiers.intersects(Modifiers::CONTROL | Modifiers::META)
    {
        return None;
    }
    Some(KeyChord::new(modifiers, code))
}

/// A chord as `chrome.commands.getAll` writes it: on macOS the
/// modifier symbols in Apple's order and the key with nothing between
/// (`⇧⌘U`), elsewhere `Ctrl+Shift+Y`. Keys other than letters and
/// digits are written with their manifest name on every platform.
pub(crate) fn shortcut_text(chord: KeyChord, platform: KeyPlatform) -> String {
    let mut text = String::new();
    let parts: [(Modifiers, &str); 4] = if platform == KeyPlatform::Mac {
        [
            (Modifiers::CONTROL, "⌃"),
            (Modifiers::ALT, "⌥"),
            (Modifiers::SHIFT, "⇧"),
            (Modifiers::META, "⌘"),
        ]
    } else {
        [
            (Modifiers::CONTROL, "Ctrl+"),
            (Modifiers::ALT, "Alt+"),
            (Modifiers::SHIFT, "Shift+"),
            (Modifiers::META, "Meta+"),
        ]
    };
    for (modifier, name) in parts {
        if chord.modifiers.contains(modifier) {
            text.push_str(name);
        }
    }
    text.push_str(&key_name(chord.code));
    text
}

/// What `chrome.commands.getAll()` resolves with: every command with
/// the shortcut it declared for this platform, `""` when it has none.
/// The shell's keymap is not asked: a chord the user's keymap holds is
/// still reported here.
pub(crate) fn get_all(commands: &[ExtensionCommand], platform: KeyPlatform) -> Value {
    Value::Array(
        commands
            .iter()
            .map(|command| {
                json!({
                    "name": command.name,
                    "description": command.description,
                    "shortcut": command
                        .chord
                        .map(|chord| shortcut_text(chord, platform))
                        .unwrap_or_default(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use keyboard_types::{Code, Modifiers};
    use osmena_core::{ExtensionCommand, KeyChord};
    use serde_json::{Value, json};

    use super::*;

    fn chord(modifiers: Modifiers, code: Code) -> Option<KeyChord> {
        Some(KeyChord::new(modifiers, code))
    }

    /// The commands of a whole manifest: the manifest's reader, then this
    /// module. What `read_manifest` does, without the files.
    fn manifest_commands(
        manifest: &Value,
        messages: Option<&Value>,
        platform: KeyPlatform,
    ) -> Vec<ExtensionCommand> {
        let facts = crate::extensions::manifest_facts(manifest, platform);
        commands_from(&facts.commands, messages, platform)
    }

    #[test]
    fn ctrl_is_command_on_macos_and_control_elsewhere() {
        let shift = Modifiers::SHIFT;
        assert_eq!(
            parse_shortcut("Ctrl+Shift+Y", KeyPlatform::Mac, true),
            chord(Modifiers::META | shift, Code::KeyY)
        );
        assert_eq!(
            parse_shortcut("Ctrl+Shift+Y", KeyPlatform::Windows, true),
            chord(Modifiers::CONTROL | shift, Code::KeyY)
        );
        assert_eq!(
            parse_shortcut("Ctrl+Shift+Y", KeyPlatform::Linux, true),
            chord(Modifiers::CONTROL | shift, Code::KeyY)
        );
        // The Mac's own names, in a `mac` key.
        assert_eq!(
            parse_shortcut("Command+E", KeyPlatform::Mac, true),
            chord(Modifiers::META, Code::KeyE)
        );
        assert_eq!(
            parse_shortcut("MacCtrl+E", KeyPlatform::Mac, true),
            chord(Modifiers::CONTROL, Code::KeyE)
        );
        assert_eq!(
            parse_shortcut("MacCtrl+Command+E", KeyPlatform::Mac, true),
            chord(Modifiers::CONTROL | Modifiers::META, Code::KeyE)
        );
        assert_eq!(
            parse_shortcut("Option+Shift+1", KeyPlatform::Mac, true),
            chord(Modifiers::ALT | shift, Code::Digit1)
        );
        assert_eq!(
            parse_shortcut("Alt+Period", KeyPlatform::Windows, true),
            chord(Modifiers::ALT, Code::Period)
        );
    }

    #[test]
    fn every_documented_key_has_a_code() {
        for (key, code) in [
            ("A", Code::KeyA),
            ("M", Code::KeyM),
            ("Z", Code::KeyZ),
            ("0", Code::Digit0),
            ("9", Code::Digit9),
            ("Comma", Code::Comma),
            ("Period", Code::Period),
            ("Home", Code::Home),
            ("End", Code::End),
            ("PageUp", Code::PageUp),
            ("PageDown", Code::PageDown),
            ("Space", Code::Space),
            ("Insert", Code::Insert),
            ("Delete", Code::Delete),
            ("Up", Code::ArrowUp),
            ("Down", Code::ArrowDown),
            ("Left", Code::ArrowLeft),
            ("Right", Code::ArrowRight),
        ] {
            assert_eq!(
                parse_shortcut(&format!("Alt+{key}"), KeyPlatform::Windows, true),
                chord(Modifiers::ALT, code),
                "{key}"
            );
        }
        for (key, code) in [
            ("MediaNextTrack", Code::MediaTrackNext),
            ("MediaPrevTrack", Code::MediaTrackPrevious),
            ("MediaPlayPause", Code::MediaPlayPause),
            ("MediaStop", Code::MediaStop),
        ] {
            assert_eq!(
                parse_shortcut(key, KeyPlatform::Windows, true),
                chord(Modifiers::empty(), code),
                "{key}"
            );
        }
    }

    #[test]
    fn what_chrome_would_not_accept_is_no_chord() {
        for (text, platform) in [
            ("", KeyPlatform::Windows),
            ("Y", KeyPlatform::Windows),
            ("Shift+Y", KeyPlatform::Windows),
            ("Ctrl+Alt+Y", KeyPlatform::Windows),
            ("Ctrl+Shift", KeyPlatform::Windows),
            ("Ctrl+Y+U", KeyPlatform::Windows),
            ("ctrl+y", KeyPlatform::Windows),
            ("Ctrl+y", KeyPlatform::Windows),
            ("Ctrl+F13", KeyPlatform::Windows),
            ("Ctrl+BracketLeft", KeyPlatform::Windows),
            ("Ctrl+MediaStop", KeyPlatform::Windows),
            // The Mac's names mean nothing elsewhere.
            ("Command+E", KeyPlatform::Windows),
            ("MacCtrl+E", KeyPlatform::Linux),
            ("Option+E", KeyPlatform::Windows),
            ("Search+E", KeyPlatform::Mac),
        ] {
            assert_eq!(parse_shortcut(text, platform, true), None, "{text:?}");
        }
    }

    // The rules below follow Chromium's own parser at tag 156.0.8078.100:
    // `ui/base/accelerators/command.cc` (`ui::Command::ParseImpl`, "ui"
    // below) as `extensions/common/command.cc` (`Command::Parse`, "ext")
    // calls it, after `NormalizeShortcutSuggestion` (ext, lines 45 to 67).

    #[test]
    fn spaces_around_a_token_are_trimmed() {
        // ui, lines 229 and 230: `base::SplitStringPiece(accelerator, "+",
        // base::TRIM_WHITESPACE, base::SPLIT_WANT_ALL)`.
        let wanted = chord(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyY);
        for text in [
            "Ctrl + Shift + Y",
            " Ctrl+Shift+Y ",
            "Ctrl+\tShift+Y\n",
            "Ctrl +Shift+ Y",
        ] {
            assert_eq!(
                parse_shortcut(text, KeyPlatform::Windows, true),
                wanted,
                "{text:?}"
            );
        }
        // A space inside a token is not around it.
        assert_eq!(
            parse_shortcut("Ctrl+Page Up", KeyPlatform::Windows, true),
            None
        );
        // An empty token is no key and no modifier (ui, lines 336 to 339).
        assert_eq!(parse_shortcut("Ctrl+ +Y", KeyPlatform::Windows, true), None);
    }

    #[test]
    fn a_shortcut_has_three_tokens_at_most() {
        // ui, lines 227 to 234: `kMaxTokenSize` is 4 and one less when
        // Ctrl+Alt is not allowed, which is how ext calls it (ext, lines
        // 257 to 259; the default of `allow_ctrl_alt` is false,
        // `ui/base/accelerators/command.h` line 90). So two modifiers and
        // a key: a third modifier is refused whatever it is.
        assert_eq!(
            parse_shortcut("MacCtrl+Command+E", KeyPlatform::Mac, true),
            chord(Modifiers::CONTROL | Modifiers::META, Code::KeyE)
        );
        for (text, platform) in [
            ("MacCtrl+Command+Shift+E", KeyPlatform::Mac),
            ("Ctrl+Shift+Alt+Y", KeyPlatform::Windows),
            // Counted as written, before a repeat is folded.
            ("Ctrl+Ctrl+Shift+Y", KeyPlatform::Windows),
            ("Ctrl+Shift+Y+", KeyPlatform::Windows),
        ] {
            assert_eq!(parse_shortcut(text, platform, true), None, "{text:?}");
        }
    }

    #[test]
    fn a_modifier_named_twice_is_that_modifier() {
        // ui, lines 240 and 241, 271 to 274: each modifier token is
        // `modifiers |= ...`, with no check that it was not set.
        assert_eq!(
            parse_shortcut("Ctrl+Ctrl+Y", KeyPlatform::Windows, true),
            chord(Modifiers::CONTROL, Code::KeyY)
        );
        assert_eq!(
            parse_shortcut("Shift+Alt+Alt", KeyPlatform::Windows, true),
            None,
            "no key"
        );
        // On macOS `Ctrl` is Command (ext, lines 58 and 59), so the two
        // names of one modifier are one.
        assert_eq!(
            parse_shortcut("Ctrl+Command+Y", KeyPlatform::Mac, true),
            chord(Modifiers::META, Code::KeyY)
        );
        assert_eq!(
            parse_shortcut("Option+Alt+Y", KeyPlatform::Mac, true),
            chord(Modifiers::ALT, Code::KeyY)
        );
    }

    #[test]
    fn alt_with_ctrl_or_command_is_refused_on_every_platform() {
        // ui, lines 55 to 60: `!allow_ctrl_alt && accelerator.IsAltDown()
        // && (accelerator.IsCtrlDown() || accelerator.IsCmdDown())`. On
        // macOS that is Option with Command (which `Ctrl` is there) as
        // well as Option with Control (`MacCtrl`).
        for (text, platform) in [
            ("Ctrl+Alt+Y", KeyPlatform::Windows),
            ("Alt+Ctrl+Y", KeyPlatform::Linux),
            ("Ctrl+Alt+Y", KeyPlatform::Mac),
            ("Command+Option+X", KeyPlatform::Mac),
            ("Command+Alt+X", KeyPlatform::Mac),
            ("MacCtrl+Option+X", KeyPlatform::Mac),
            ("MacCtrl+Alt+X", KeyPlatform::Mac),
        ] {
            assert_eq!(parse_shortcut(text, platform, true), None, "{text:?}");
        }
        // Option alone, and with Shift, is a shortcut (ui, lines 62 to 65).
        assert_eq!(
            parse_shortcut("Option+X", KeyPlatform::Mac, true),
            chord(Modifiers::ALT, Code::KeyX)
        );
        assert_eq!(
            parse_shortcut("Alt+Shift+X", KeyPlatform::Mac, true),
            chord(Modifiers::ALT | Modifiers::SHIFT, Code::KeyX)
        );
    }

    #[test]
    fn a_media_key_is_no_shortcut_for_an_action_command() {
        // ui, lines 320 to 327: a media key is taken only with
        // `should_parse_media_keys`, which ext passes as
        // `!IsActionRelatedCommand(command_name)` (ext, line 258).
        for key in [
            "MediaNextTrack",
            "MediaPrevTrack",
            "MediaPlayPause",
            "MediaStop",
        ] {
            assert!(
                parse_shortcut(key, KeyPlatform::Mac, true).is_some(),
                "{key}"
            );
            assert_eq!(parse_shortcut(key, KeyPlatform::Mac, false), None, "{key}");
        }
        // Any other shortcut is the same for both.
        assert_eq!(
            parse_shortcut("Ctrl+Shift+U", KeyPlatform::Windows, false),
            chord(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyU)
        );
        let manifest = json!({"commands": {
            "_execute_action": {"suggested_key": "MediaStop"},
            "_execute_browser_action": {"suggested_key": "MediaPlayPause"},
            "stop": {"suggested_key": "MediaStop", "description": "Stop"}
        }});
        let commands = manifest_commands(&manifest, None, KeyPlatform::Mac);
        assert_eq!(
            commands
                .iter()
                .map(|c| (c.name.as_str(), c.chord))
                .collect::<Vec<_>>(),
            [
                ("_execute_action", None),
                ("_execute_browser_action", None),
                ("stop", chord(Modifiers::empty(), Code::MediaStop)),
            ]
        );
    }

    #[test]
    fn the_rules_chromiums_parser_shares_with_this_one_hold() {
        // A key of one character is A to Z or 0 to 9 (ui, lines 275 and
        // 328 to 335): a dot or a comma is written `Period` or `Comma`.
        for text in ["Alt+.", "Alt+,", "Alt+;", "Alt+a", "Alt+é"] {
            assert_eq!(
                parse_shortcut(text, KeyPlatform::Windows, true),
                None,
                "{text:?}"
            );
        }
        assert_eq!(
            parse_shortcut("Alt+Comma", KeyPlatform::Windows, true),
            chord(Modifiers::ALT, Code::Comma)
        );
        // A media key takes no modifier (ui, lines 349 to 354).
        assert_eq!(
            parse_shortcut("Ctrl+MediaStop", KeyPlatform::Windows, true),
            None
        );
        // Shift needs Ctrl, Alt or Command beside it (ui, lines 62 to 65),
        // and every other key needs a modifier (ui, lines 50 to 53).
        assert_eq!(
            parse_shortcut("Shift+Tab", KeyPlatform::Windows, true),
            None
        );
        assert_eq!(parse_shortcut("Tab", KeyPlatform::Windows, true), None);
        // Two keys (ui, lines 286 to 290).
        assert_eq!(parse_shortcut("Ctrl+Y+U", KeyPlatform::Windows, true), None);
        // The key may stand anywhere among the tokens (ui, line 239).
        assert_eq!(
            parse_shortcut("Y+Ctrl", KeyPlatform::Windows, true),
            chord(Modifiers::CONTROL, Code::KeyY)
        );
    }

    #[test]
    fn the_manifest_gives_each_command_its_platform_chord() {
        let manifest = json!({
            "commands": {
                "toggle": {
                    "suggested_key": {"default": "Ctrl+E", "mac": "Command+E", "linux": "Alt+E"},
                    "description": "Toggle the panel"
                },
                "_execute_action": {"suggested_key": {"default": "Ctrl+Shift+U"}},
                "plain": {"suggested_key": "Alt+Shift+P", "description": "  "},
                "unbound": {"description": "No key"},
                "bad": {"suggested_key": {"default": "Shift+B"}},
                "windows-only": {"suggested_key": {"windows": "Ctrl+Shift+9"}}
            }
        });
        let named = |commands: &[ExtensionCommand], name: &str| {
            commands
                .iter()
                .find(|c| c.name == name)
                .map(|c| (c.description.clone(), c.chord))
                .unwrap()
        };
        let shift = Modifiers::SHIFT;

        let mac = manifest_commands(&manifest, None, KeyPlatform::Mac);
        assert_eq!(
            mac.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            [
                "_execute_action",
                "bad",
                "plain",
                "toggle",
                "unbound",
                "windows-only"
            ]
        );
        assert_eq!(
            named(&mac, "toggle"),
            (
                "Toggle the panel".to_string(),
                chord(Modifiers::META, Code::KeyE)
            )
        );
        assert_eq!(
            named(&mac, "_execute_action"),
            (String::new(), chord(Modifiers::META | shift, Code::KeyU))
        );
        assert_eq!(
            named(&mac, "plain"),
            (String::new(), chord(Modifiers::ALT | shift, Code::KeyP))
        );
        assert_eq!(named(&mac, "unbound"), ("No key".to_string(), None));
        assert_eq!(named(&mac, "bad"), (String::new(), None));
        assert_eq!(named(&mac, "windows-only"), (String::new(), None));

        let windows = manifest_commands(&manifest, None, KeyPlatform::Windows);
        assert_eq!(
            named(&windows, "toggle").1,
            chord(Modifiers::CONTROL, Code::KeyE)
        );
        assert_eq!(
            named(&windows, "windows-only").1,
            chord(Modifiers::CONTROL | shift, Code::Digit9)
        );

        let linux = manifest_commands(&manifest, None, KeyPlatform::Linux);
        assert_eq!(named(&linux, "toggle").1, chord(Modifiers::ALT, Code::KeyE));
    }

    #[test]
    fn a_chord_two_commands_suggest_goes_to_the_first() {
        let manifest = json!({"commands": {
            "a": {"suggested_key": "Ctrl+Shift+Y"},
            "b": {"suggested_key": "Ctrl+Shift+Y"}
        }});
        let commands = manifest_commands(&manifest, None, KeyPlatform::Windows);
        assert!(commands[0].chord.is_some());
        assert_eq!(commands[1].chord, None);
    }

    #[test]
    fn a_manifest_without_commands_has_none() {
        for manifest in [json!({}), json!({"commands": []}), json!({"commands": "x"})] {
            assert_eq!(manifest_commands(&manifest, None, KeyPlatform::Mac), vec![]);
        }
    }

    #[test]
    fn a_localised_description_is_read_from_the_messages() {
        let messages = json!({
            "commandToggle": {"message": "Toggle Claude", "description": "…"},
            "empty": {}
        });
        assert_eq!(localized("Plain", Some(&messages)), "Plain");
        assert_eq!(
            localized("__MSG_commandToggle__", Some(&messages)),
            "Toggle Claude"
        );
        // Chrome matches message names without regard to case.
        assert_eq!(
            localized("__MSG_COMMANDTOGGLE__", Some(&messages)),
            "Toggle Claude"
        );
        assert_eq!(localized("__MSG_empty__", Some(&messages)), "");
        assert_eq!(localized("__MSG_missing__", Some(&messages)), "");
        assert_eq!(localized("__MSG_commandToggle__", None), "");

        let manifest = json!({"commands": {"toggle": {"description": "__MSG_commandToggle__"}}});
        let commands = manifest_commands(&manifest, Some(&messages), KeyPlatform::Mac);
        assert_eq!(commands[0].description, "Toggle Claude");
    }

    #[test]
    fn the_action_commands_are_the_three_reserved_names() {
        assert!(is_action_command("_execute_action"));
        assert!(is_action_command("_execute_browser_action"));
        assert!(is_action_command("_execute_page_action"));
        assert!(!is_action_command("_execute_side_panel"));
        assert!(!is_action_command("toggle"));
    }

    #[test]
    fn get_all_reports_the_shortcut_the_way_chrome_writes_it() {
        let shift = Modifiers::SHIFT;
        // As Chrome on macOS answered a probe extension.
        assert_eq!(
            shortcut_text(
                KeyChord::new(Modifiers::META | shift, Code::KeyU),
                KeyPlatform::Mac
            ),
            "⇧⌘U"
        );
        assert_eq!(
            shortcut_text(
                KeyChord::new(
                    Modifiers::CONTROL | Modifiers::ALT | shift | Modifiers::META,
                    Code::Digit1
                ),
                KeyPlatform::Mac
            ),
            "⌃⌥⇧⌘1"
        );
        assert_eq!(
            shortcut_text(
                KeyChord::new(Modifiers::CONTROL | shift, Code::KeyY),
                KeyPlatform::Windows
            ),
            "Ctrl+Shift+Y"
        );
        assert_eq!(
            shortcut_text(
                KeyChord::new(Modifiers::ALT, Code::Period),
                KeyPlatform::Linux
            ),
            "Alt+Period"
        );

        let commands = [
            ExtensionCommand {
                name: "_execute_action".to_string(),
                description: String::new(),
                chord: Some(KeyChord::new(Modifiers::META | shift, Code::KeyU)),
            },
            ExtensionCommand {
                name: "unbound".to_string(),
                description: "No key".to_string(),
                chord: None,
            },
        ];
        assert_eq!(
            get_all(&commands, KeyPlatform::Mac),
            json!([
                {"name": "_execute_action", "description": "", "shortcut": "⇧⌘U"},
                {"name": "unbound", "description": "No key", "shortcut": ""}
            ])
        );
        // The Claude extension's one command, as its options page reads it.
        let claude = manifest_commands(
            &json!({"commands": {"toggle-side-panel": {
                "suggested_key": {"default": "Ctrl+E", "mac": "Command+E"},
                "description": "Toggle Claude side panel"
            }}}),
            None,
            KeyPlatform::Mac,
        );
        assert_eq!(
            get_all(&claude, KeyPlatform::Mac),
            json!([{"name": "toggle-side-panel", "description": "Toggle Claude side panel",
                    "shortcut": "⌘E"}])
        );
    }
}
