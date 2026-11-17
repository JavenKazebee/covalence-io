//! Matching devices to plugins' manifests (plugin protocol §7.1).

use std::collections::BTreeMap;

use covalence_protocol::io::{DeviceInfo, DeviceKind, DeviceMatch};
use serde::Serialize;

/// Whether `m` covers `device`: every field `m` gives must agree.
pub fn matches(m: &DeviceMatch, device: &DeviceInfo) -> bool {
    m.kind == device.kind
        && m.vendor_id.is_none_or(|v| device.vendor_id == Some(v))
        && m.product_id
            .as_ref()
            .is_none_or(|p| device.product_id.is_some_and(|id| p.contains(id)))
        && (m.kind != DeviceKind::Hid || m.usage_page.is_none_or(|u| device.usage_page == Some(u)))
        && m.name
            .as_deref()
            .is_none_or(|g| device.product.as_deref().is_some_and(|p| glob(g, p)))
}

/// A plugin's `devices`, as matching sees them.
#[derive(Clone, Copy, Debug)]
pub struct PluginDevices<'a> {
    pub plugin: &'a str,
    pub matches: &'a [DeviceMatch],
}

/// Devices any of `matches` cover, of `kind` if given: what `device.list`
/// answers and a connection form's picker offers.
pub fn matching<'a>(
    matches: &[DeviceMatch],
    kind: Option<DeviceKind>,
    devices: impl IntoIterator<Item = &'a DeviceInfo>,
) -> Vec<DeviceInfo> {
    devices
        .into_iter()
        .filter(|d| kind.is_none_or(|k| d.kind == k))
        .filter(|d| matches.iter().any(|m| self::matches(m, d)))
        .cloned()
        .collect()
}

/// What to do about a device no connection holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Claim {
    /// Offer a connection with one of `plugins`, in the UI.
    Offer {
        device: String,
        plugins: Vec<String>,
    },
    /// Create a connection with `plugin` without asking.
    AutoCreate { device: String, plugin: String },
}

impl Claim {
    pub fn device(&self) -> &str {
        match self {
            Claim::Offer { device, .. } | Claim::AutoCreate { device, .. } => device,
        }
    }
}

/// The claims on every device that `held` says no connection holds.
///
/// A device is created without asking only when exactly one plugin matches it
/// with `autoCreate`. One that several plugins match with `autoCreate`, or
/// none, is offered once, with every plugin that matches it to choose from.
pub fn claims<'a>(
    plugins: &[PluginDevices<'_>],
    devices: impl IntoIterator<Item = &'a DeviceInfo>,
    held: impl Fn(&str) -> bool,
) -> Vec<Claim> {
    let mut out = Vec::new();
    for device in devices {
        if held(&device.id) {
            continue;
        }
        // Plugin → whether any of its matches says autoCreate.
        let mut by_plugin: BTreeMap<&str, bool> = BTreeMap::new();
        for p in plugins {
            for m in p.matches.iter().filter(|m| matches(m, device)) {
                *by_plugin.entry(p.plugin).or_default() |= m.auto_create == Some(true);
            }
        }
        if by_plugin.is_empty() {
            continue;
        }
        let auto: Vec<&str> = by_plugin
            .iter()
            .filter(|(_, auto)| **auto)
            .map(|(p, _)| *p)
            .collect();
        out.push(match auto.as_slice() {
            [plugin] => Claim::AutoCreate {
                device: device.id.clone(),
                plugin: (*plugin).to_owned(),
            },
            _ => Claim::Offer {
                device: device.id.clone(),
                plugins: by_plugin.keys().map(|p| (*p).to_owned()).collect(),
            },
        });
    }
    out
}

/// A case-insensitive glob: `*` is any run of characters, `?` any one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    // Iterative matching with one backtrack point, the last `*`.
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(&c) if c == '?' || c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use covalence_protocol::io::ProductIds;

    fn deck(id: &str, product: u16) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            kind: DeviceKind::Hid,
            vendor_id: Some(0x0fd9),
            product_id: Some(product),
            serial: None,
            manufacturer: Some("Elgato".into()),
            product: Some("Stream Deck MK.2".into()),
            path: format!("/dev/{id}"),
            usage_page: Some(0x000c),
            usage: Some(1),
        }
    }

    fn elgato(auto: bool) -> DeviceMatch {
        DeviceMatch {
            kind: DeviceKind::Hid,
            vendor_id: Some(0x0fd9),
            product_id: Some(ProductIds::Many(vec![0x0080, 0x00a5])),
            usage_page: None,
            name: None,
            auto_create: Some(auto),
        }
    }

    #[test]
    fn globs() {
        assert!(glob("Stream Deck*", "stream deck mk.2"));
        assert!(glob("*MK.?", "Stream Deck MK.2"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "aXbYbZc"));
        assert!(!glob("Stream Deck", "Stream Deck MK.2"));
        assert!(!glob("?", ""));
    }

    #[test]
    fn every_given_field_must_agree() {
        let d = deck("a", 0x0080);
        assert!(matches(&elgato(false), &d));
        assert!(!matches(&elgato(false), &deck("b", 0x0060)));
        let named = DeviceMatch {
            name: Some("*mk.2".into()),
            usage_page: Some(0x000c),
            ..elgato(false)
        };
        assert!(matches(&named, &d));
        assert!(!matches(
            &DeviceMatch {
                usage_page: Some(0xff00),
                ..named.clone()
            },
            &d
        ));
        assert!(!matches(
            &DeviceMatch {
                kind: DeviceKind::Serial,
                ..named
            },
            &d
        ));
    }

    #[test]
    fn auto_creates_only_when_one_plugin_says_so() {
        let auto = [elgato(true)];
        let plain = [elgato(false)];
        let devices = [deck("a", 0x0080), deck("b", 0x0080), deck("c", 0x0060)];
        let streamdeck = PluginDevices {
            plugin: "io.covalence.streamdeck",
            matches: &auto,
        };
        let generic = PluginDevices {
            plugin: "io.example.hid",
            matches: &plain,
        };

        let got = claims(&[streamdeck, generic], &devices, |id| id == "b");
        assert_eq!(
            got,
            [Claim::AutoCreate {
                device: "a".into(),
                plugin: "io.covalence.streamdeck".into()
            }]
        );

        let rival = PluginDevices {
            plugin: "io.example.deck",
            matches: &auto,
        };
        let got = claims(&[streamdeck, rival, generic], &devices[..1], |_| false);
        assert_eq!(
            got,
            [Claim::Offer {
                device: "a".into(),
                plugins: vec![
                    "io.covalence.streamdeck".into(),
                    "io.example.deck".into(),
                    "io.example.hid".into()
                ],
            }]
        );

        let got = claims(&[generic], &devices, |_| false);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|c| matches!(c, Claim::Offer { .. })));
    }
}
