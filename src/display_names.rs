use serde::{Deserialize, Serialize};

/// Presentation only: never applied to filenames or persisted score metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayNames {
    pub replace_characters: bool,
    pub characters: String,
    pub remove_imslp_prefix: bool,
}

impl Default for DisplayNames {
    fn default() -> Self {
        Self {
            replace_characters: false,
            characters: "_".into(),
            remove_imslp_prefix: false,
        }
    }
}

impl DisplayNames {
    pub fn format(&self, original: &str) -> String {
        if !self.replace_characters && !self.remove_imslp_prefix {
            return original.to_owned();
        }
        let title = if self.remove_imslp_prefix {
            remove_prefix(original).unwrap_or(original)
        } else {
            original
        };
        let replaced: String = title
            .chars()
            .map(|c| {
                if self.replace_characters && self.characters.contains(c) {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let formatted = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
        if formatted.is_empty() {
            original.to_owned()
        } else {
            formatted
        }
    }
}

fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.')
}

fn identifier<'a>(input: &'a str, tag: &str) -> Option<&'a str> {
    if !input.get(..tag.len())?.eq_ignore_ascii_case(tag) {
        return None;
    }
    let digits = input[tag.len()..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if digits == 0 {
        return None;
    }
    let remainder = &input[tag.len() + digits..];
    if remainder
        .chars()
        .next()
        .is_some_and(|c| !separator(c) && c != ']' && c != ')')
    {
        return None;
    }
    Some(remainder)
}

fn remove_prefix(original: &str) -> Option<&str> {
    let input = original.trim_start();
    let (input, closing) = match input.chars().next() {
        Some('[') => (&input[1..], Some(']')),
        Some('(') => (&input[1..], Some(')')),
        _ => (input, None),
    };
    let mut remainder = if let Some(rest) = identifier(input, "IMSLP") {
        let rest = rest.trim_start_matches(separator);
        identifier(rest, "PMLP").unwrap_or(rest)
    } else {
        identifier(input, "PMLP")?
    };
    remainder = remainder.trim_start_matches(separator);
    if let Some(closing) = closing {
        remainder = remainder.strip_prefix(closing)?;
    } else if remainder.starts_with([']', ')']) {
        return None;
    }
    remainder = remainder.trim_start_matches(separator);
    if remainder.is_empty() {
        None
    } else {
        Some(remainder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> DisplayNames {
        DisplayNames {
            replace_characters: true,
            remove_imslp_prefix: true,
            ..Default::default()
        }
    }

    #[test]
    fn cleanup_is_opt_in_and_options_are_independent() {
        let name = "IMSLP237087-PMLP384177-Bach_Prelude";
        assert_eq!(DisplayNames::default().format(name), name);
        let mut options = enabled();
        options.remove_imslp_prefix = false;
        assert_eq!(options.format(name), "IMSLP237087-PMLP384177-Bach Prelude");
        options.remove_imslp_prefix = true;
        options.replace_characters = false;
        assert_eq!(options.format(name), "Bach_Prelude");
    }

    #[test]
    fn old_preferences_and_partially_saved_options_load_with_defaults() {
        let old: crate::model::Preferences =
            serde_json::from_str(r#"{"next_key":"N","previous_key":"P","recursive":false}"#)
                .unwrap();
        assert_eq!(old.next_key, "N");
        assert!(!old.display_names.replace_characters);
        assert!(!old.display_names.remove_imslp_prefix);
        assert_eq!(old.display_names.characters, "_");
        assert!(
            !old.keep_awake,
            "Keep-awake must be opt-in for old preferences"
        );
        let partial: DisplayNames =
            serde_json::from_str(r#"{"remove_imslp_prefix":true}"#).unwrap();
        assert_eq!(partial.characters, "_");
        assert!(!partial.replace_characters);
        assert!(partial.remove_imslp_prefix);
    }

    #[test]
    fn recognizes_only_leading_imslp_identifiers() {
        for name in [
            "IMSLP237087-PMLP384177-Bach_Prelude",
            "IMSLP237087_PMLP384177__Bach_Prelude",
            "imslp237087 - pmlp384177 - Bach_Prelude",
            "[IMSLP237087-PMLP384177] Bach_Prelude",
            "(IMSLP237087-PMLP384177)_Bach_Prelude",
            "IMSLP237087-Bach_Prelude",
            "PMLP384177-Bach_Prelude",
        ] {
            assert_eq!(enabled().format(name), "Bach Prelude", "{name}");
        }
        for name in [
            "Op. 237087 - Prelude",
            "Bach IMSLP237087-PMLP384177",
            "IMSLP collection",
            "IMSLP123Symphony",
            "IMSLP237087-PMLP384177",
            "[IMSLP237087-PMLP384177-Bach",
            "IMSLP237087] Bach",
            "作品237087",
        ] {
            assert_eq!(enabled().format(name), name, "{name}");
        }
    }

    #[test]
    fn supports_custom_unicode_characters_and_preserves_musical_punctuation() {
        let mut options = enabled();
        options.characters = "_·".into();
        assert_eq!(
            options.format("IMSLP123-PMLP456-Étude__Op.10·No.1-in-C#"),
            "Étude Op.10 No.1-in-C#"
        );
        assert_eq!(options.format("___"), "___");
        options.characters.clear();
        assert_eq!(options.format("Bach_Prelude"), "Bach_Prelude");
    }
}
