//! Keyboard layout mapping for non-Latin layouts (Russian JCUKEN and Uzbek Cyrillic).
//!
//! Slint's `KeyEvent` exposes only the generated text (not physical keycodes).
//! When a Russian or Uzbek Cyrillic layout is active, shortcut key combinations
//! like Ctrl+C or bare keys like P/L/A emit Cyrillic characters instead of ASCII letters.
//! This module maps those Cyrillic characters to their corresponding QWERTY positions.

/// Maps a character from Russian JCUKEN or Uzbek Cyrillic layouts to its QWERTY Latin equivalent.
pub fn map_cyrillic_to_qwerty(c: char) -> char {
    match c {
        // C: Russian & Uzbek 'с', 'С'
        'с' | 'С' => 'c',
        // S: Russian 'ы', 'Ы', Uzbek 'ҳ', 'Ҳ'
        'ы' | 'Ы' | 'ҳ' | 'Ҳ' => 's',
        // Z: Russian & Uzbek 'я', 'Я'
        'я' | 'Я' => 'z',
        // A: Russian & Uzbek 'ф', 'Ф'
        'ф' | 'Ф' => 'a',
        // Y: Russian & Uzbek 'н', 'Н'
        'н' | 'Н' => 'y',
        // P: Russian & Uzbek 'з', 'З'
        'з' | 'З' => 'p',
        // L: Russian & Uzbek 'д', 'Д'
        'д' | 'Д' => 'l',
        // R: Russian & Uzbek 'к', 'К'
        'к' | 'К' => 'r',
        // M: Russian 'ь', 'Ь', Uzbek 'ь', 'Ь', 'ғ', 'Ғ'
        'ь' | 'Ь' | 'ғ' | 'Ғ' => 'm',
        // B: Russian & Uzbek 'и', 'И'
        'и' | 'И' => 'b',

        // Standard Latin uppercase to lowercase normalization
        'A'..='Z' => c.to_ascii_lowercase(),
        _ => c,
    }
}

/// Normalizes Slint key event text into a single lowercase command character.
pub fn normalize_key_text(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        Some(map_cyrillic_to_qwerty(first))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_russian_jcuken_lowercase() {
        assert_eq!(map_cyrillic_to_qwerty('с'), 'c');
        assert_eq!(map_cyrillic_to_qwerty('ы'), 's');
        assert_eq!(map_cyrillic_to_qwerty('я'), 'z');
        assert_eq!(map_cyrillic_to_qwerty('ф'), 'a');
        assert_eq!(map_cyrillic_to_qwerty('н'), 'y');
        assert_eq!(map_cyrillic_to_qwerty('з'), 'p');
        assert_eq!(map_cyrillic_to_qwerty('д'), 'l');
        assert_eq!(map_cyrillic_to_qwerty('к'), 'r');
        assert_eq!(map_cyrillic_to_qwerty('ь'), 'm');
        assert_eq!(map_cyrillic_to_qwerty('и'), 'b');
    }

    #[test]
    fn test_russian_jcuken_uppercase() {
        assert_eq!(map_cyrillic_to_qwerty('С'), 'c');
        assert_eq!(map_cyrillic_to_qwerty('Ы'), 's');
        assert_eq!(map_cyrillic_to_qwerty('Я'), 'z');
        assert_eq!(map_cyrillic_to_qwerty('Ф'), 'a');
        assert_eq!(map_cyrillic_to_qwerty('Н'), 'y');
        assert_eq!(map_cyrillic_to_qwerty('З'), 'p');
        assert_eq!(map_cyrillic_to_qwerty('Д'), 'l');
        assert_eq!(map_cyrillic_to_qwerty('К'), 'r');
        assert_eq!(map_cyrillic_to_qwerty('Ь'), 'm');
        assert_eq!(map_cyrillic_to_qwerty('И'), 'b');
    }

    #[test]
    fn test_uzbek_cyrillic_equivalents() {
        // Uzbek 'ҳ' / 'Ҳ' on S key
        assert_eq!(map_cyrillic_to_qwerty('ҳ'), 's');
        assert_eq!(map_cyrillic_to_qwerty('Ҳ'), 's');
        // Uzbek 'ғ' / 'Ғ' on M key
        assert_eq!(map_cyrillic_to_qwerty('ғ'), 'm');
        assert_eq!(map_cyrillic_to_qwerty('Ғ'), 'm');
    }

    #[test]
    fn test_ascii_passthrough_and_case() {
        assert_eq!(map_cyrillic_to_qwerty('c'), 'c');
        assert_eq!(map_cyrillic_to_qwerty('C'), 'c');
        assert_eq!(map_cyrillic_to_qwerty('z'), 'z');
        assert_eq!(map_cyrillic_to_qwerty('Z'), 'z');
        assert_eq!(map_cyrillic_to_qwerty('1'), '1');
    }

    #[test]
    fn test_normalize_key_text() {
        assert_eq!(normalize_key_text("с"), Some('c'));
        assert_eq!(normalize_key_text("С"), Some('c'));
        assert_eq!(normalize_key_text("C"), Some('c'));
        assert_eq!(normalize_key_text("c"), Some('c'));
        assert_eq!(normalize_key_text(""), None);
        assert_eq!(normalize_key_text("Escape"), None);
    }
}
