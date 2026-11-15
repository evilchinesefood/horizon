//! Opt-in enchantment descriptions supplied by an enabled content package.
use serde::Deserialize;
use std::{io::ErrorKind, path::Path};

const DECLARATION: &str = "assets/enchdesc/horizon/enchantment_descriptions.json";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Stored,
    Applied,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rules {
    enabled: bool,
    require_crouch: bool,
    subjects: Vec<Subject>,
    color: [u8; 3],
    max_width: u16,
}

#[derive(Default, Debug)]
pub struct EnchantmentDescriptions(Option<Rules>);

pub struct Description {
    pub text: String,
    pub color: u32,
    pub max_width: f32,
}

impl EnchantmentDescriptions {
    pub fn load(root: &Path) -> crate::Result<Self> {
        match std::fs::read(root.join(DECLARATION)) {
            Ok(bytes) => Self::parse(&bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn parse(bytes: &[u8]) -> crate::Result<Self> {
        let rules: Rules = serde_json::from_slice(bytes)?;
        if !(40..=320).contains(&rules.max_width) {
            return Err(
                "enchantment description max_width must be between 40 and 320 pixels".into(),
            );
        }
        Ok(Self(rules.enabled.then_some(rules)))
    }

    pub fn description(
        &self,
        enchantment: &str,
        subject: Subject,
        crouch_down: bool,
        translate: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Description> {
        let rules = self.0.as_ref()?;
        if !rules.subjects.contains(&subject) || (rules.require_crouch && !crouch_down) {
            return None;
        }
        let (namespace, path) = enchantment
            .split_once(':')
            .unwrap_or(("minecraft", enchantment));
        let text = translate(&format!(
            "enchantment.{namespace}.{}.desc",
            path.replace('/', ".")
        ))?;
        if text.trim().is_empty() {
            return None;
        }
        Some(Description {
            text,
            color: crate::hud::rgba(rules.color[0], rules.color[1], rules.color[2], 255),
            max_width: f32::from(rules.max_width),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const RULES: &[u8] = br#"{"enabled":true,"require_crouch":false,"subjects":["stored","applied"],"color":[170,170,170],"max_width":180}"#;

    #[test]
    fn books_and_equipment_use_active_translation_and_missing_keys_stay_absent() {
        let provider = EnchantmentDescriptions::parse(RULES).unwrap();
        for subject in [Subject::Stored, Subject::Applied] {
            let description = provider
                .description("example:frost", subject, false, &|key| {
                    assert_eq!(key, "enchantment.example.frost.desc");
                    Some("Slows the target.".into())
                })
                .unwrap();
            assert_eq!(description.text, "Slows the target.");
            assert_eq!(description.max_width, 180.0);
            assert_eq!(description.color, 0xffaaaaaa);
            assert!(
                provider
                    .description("example:missing", subject, false, &|_| None)
                    .is_none()
            );
        }
        let translated = provider
            .description("example:frost", Subject::Stored, false, &|_| {
                Some("Verlangsamt das Ziel.".into())
            })
            .unwrap();
        assert_eq!(translated.text, "Verlangsamt das Ziel.");
    }

    #[test]
    fn crouch_and_scope_are_enforced_before_reading_descriptions() {
        let provider = EnchantmentDescriptions::parse(br#"{"enabled":true,"require_crouch":true,"subjects":["stored"],"color":[255,255,255],"max_width":100}"#).unwrap();
        let translate = |_: &str| Some("Description".into());
        assert!(
            provider
                .description("minecraft:mending", Subject::Stored, false, &translate)
                .is_none()
        );
        assert!(
            provider
                .description("minecraft:mending", Subject::Applied, true, &translate)
                .is_none()
        );
        assert_eq!(
            provider
                .description("minecraft:mending", Subject::Stored, true, &translate)
                .unwrap()
                .text,
            "Description"
        );
    }

    #[test]
    fn reload_removes_disabled_or_removed_provider_and_rejects_bad_rules() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "horizon-enchdesc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = root.join(DECLARATION);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let lookup = |_: &str| Some("Description".into());
        let visible = |p: EnchantmentDescriptions| {
            p.description("minecraft:mending", Subject::Stored, false, &lookup)
                .is_some()
        };
        assert!(!visible(EnchantmentDescriptions::load(&root).unwrap()));
        std::fs::write(&file, RULES).unwrap();
        assert!(visible(EnchantmentDescriptions::load(&root).unwrap()));
        std::fs::write(
            &file,
            String::from_utf8_lossy(RULES).replace("true", "false"),
        )
        .unwrap();
        assert!(!visible(EnchantmentDescriptions::load(&root).unwrap()));
        std::fs::write(&file, b"{}").unwrap();
        assert!(EnchantmentDescriptions::load(&root).is_err());
        std::fs::remove_file(&file).unwrap();
        assert!(!visible(EnchantmentDescriptions::load(&root).unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn language_fallback_and_resource_override_reach_descriptions() {
        struct Source(&'static str, &'static str);
        impl crate::lang::ResourceSource for Source {
            fn read(&self, path: &str) -> Option<Vec<u8>> {
                match path {
                    "assets/enchdesc/lang/en_us.json" => Some(self.0.as_bytes().to_vec()),
                    "assets/enchdesc/lang/de_de.json" => Some(self.1.as_bytes().to_vec()),
                    _ => None,
                }
            }
            fn namespaces(&self) -> Vec<String> {
                vec!["enchdesc".into()]
            }
        }
        let base = Source(
            r#"{"enchantment.minecraft.mending.desc":"Repairs using experience.","enchantment.minecraft.thorns.desc":"Hurts attackers."}"#,
            r#"{"enchantment.minecraft.mending.desc":"Repariert mit Erfahrung."}"#,
        );
        let override_pack = Source(
            "{}",
            r#"{"enchantment.minecraft.mending.desc":"Pack replacement."}"#,
        );
        let provider = EnchantmentDescriptions::parse(RULES).unwrap();
        let language = crate::lang::Language::load(&[&base], "de_de");
        let lookup = |key: &str| language.get(key).map(str::to_owned);
        assert_eq!(
            provider
                .description("minecraft:mending", Subject::Stored, false, &lookup)
                .unwrap()
                .text,
            "Repariert mit Erfahrung."
        );
        assert_eq!(
            provider
                .description("minecraft:thorns", Subject::Applied, false, &lookup)
                .unwrap()
                .text,
            "Hurts attackers."
        );
        let language = crate::lang::Language::load(&[&base, &override_pack], "de_de");
        assert_eq!(
            provider
                .description("minecraft:mending", Subject::Stored, false, &|key| language
                    .get(key)
                    .map(str::to_owned))
                .unwrap()
                .text,
            "Pack replacement."
        );
    }

    #[test]
    fn declaration_rejects_unknown_settings_and_unusable_widths() {
        let text = String::from_utf8_lossy(RULES);
        assert!(EnchantmentDescriptions::parse(text.replace("180", "0").as_bytes()).is_err());
        assert!(EnchantmentDescriptions::parse(text.replace("180", "400").as_bytes()).is_err());
        assert!(
            EnchantmentDescriptions::parse(text.replace("\"enabled\"", "\"enabeld\"").as_bytes())
                .is_err()
        );
    }
}
