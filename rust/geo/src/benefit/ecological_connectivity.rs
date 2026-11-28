//! 5.7 Maintenance of ecological connectivity (`benefit/ecological_connectivity.py`): MSPA
//! classes per pathway on the pathway grid (ESRI:54034 warp, `load_raster_clipped`).

use serde_json::{Value, json};

use super::config::{
    CONNECTIVITY_ECO_WORDS, MANAGE_CODE, MSPA_2024_RASTER, MSPA_BACKGROUND_CODES, MSPA_CLASS_CODES,
    MSPA_CONNECTOR_CLASSES, MSPA_CORE_CLASSES, MSPA_EDGE_CLASSES, MSPA_HABITAT_CLASSES,
    PROTECT_CODE, RESTORE_CODE, lookup,
};
use super::util::na;
use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_clipped};
use crate::config::{PATHWAY_BAND, PATHWAY_ECOSYSTEM_BAND, PATHWAY_RASTER};
use crate::py::{fmt_ha, fmt_pct, oxford_join, round_ndigits, safe_pct};
use crate::settings::Settings;

/// `_count_habitat_patches`: the number of 8-connected components of `core` (what
/// `rasterio.features.shapes(core, mask=core, connectivity=8)` yields with value 1).
fn count_habitat_patches(core: &[bool], width: usize, height: usize) -> usize {
    let mut seen = vec![false; core.len()];
    let mut stack = Vec::new();
    let mut n = 0;
    for start in 0..core.len() {
        if !core[start] || seen[start] {
            continue;
        }
        n += 1;
        seen[start] = true;
        stack.push(start);
        while let Some(p) = stack.pop() {
            let (r, c) = ((p / width) as i64, (p % width) as i64);
            for dr in -1..=1 {
                for dc in -1..=1 {
                    let (rr, cc) = (r + dr, c + dc);
                    if rr < 0 || cc < 0 || rr >= height as i64 || cc >= width as i64 {
                        continue;
                    }
                    let q = rr as usize * width + cc as usize;
                    if core[q] && !seen[q] {
                        seen[q] = true;
                        stack.push(q);
                    }
                }
            }
        }
    }
    n
}

fn ints(values: &[f64], mask: &[bool]) -> Vec<i64> {
    values
        .iter()
        .zip(mask)
        .map(|(v, m)| if *m { 0 } else { *v as i64 })
        .collect()
}

fn and(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b).map(|(x, y)| *x && *y).collect()
}

fn count(m: &[bool]) -> usize {
    m.iter().filter(|x| **x).count()
}

/// Component 5.7.
pub fn analyze_ecological_connectivity(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
) -> Result<(Value, Value)> {
    let pathway = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_BAND,
        None,
    )?;
    let grid = pathway.grid();
    let mspa = load_raster_clipped(
        settings,
        MSPA_2024_RASTER,
        aoi,
        Resampling::Nearest,
        1,
        Some(grid),
    )?;
    let ecosystem = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_ECOSYSTEM_BAND,
        Some(grid),
    )?;
    let pix = pathway.pixel_area_ha;
    let code = ints(&mspa.values, &mspa.mask);
    let valid: Vec<bool> = mspa.mask.iter().map(|m| !m).collect();
    let cls_mask = |names: &[&str]| -> Vec<bool> {
        let codes: Vec<i64> = MSPA_CLASS_CODES
            .iter()
            .filter(|(_, n)| names.contains(n))
            .map(|(c, _)| *c)
            .collect();
        code.iter()
            .zip(&valid)
            .map(|(c, v)| *v && codes.contains(c))
            .collect()
    };
    let habitat = cls_mask(&MSPA_HABITAT_CLASSES);
    let core = cls_mask(&MSPA_CORE_CLASSES);
    let connector = cls_mask(&MSPA_CONNECTOR_CLASSES);
    let edge = cls_mask(&MSPA_EDGE_CLASSES);
    if !habitat.iter().any(|h| *h) {
        return Ok(na(
            "No mapped MSPA habitat covers this project area, so connectivity cannot be assessed.",
        ));
    }
    let pw = ints(&pathway.values, &pathway.mask);
    let protect: Vec<bool> = pw.iter().map(|p| *p as f64 == PROTECT_CODE).collect();
    let manage: Vec<bool> = pw.iter().map(|p| *p == MANAGE_CODE).collect();
    let restore: Vec<bool> = pw.iter().map(|p| *p as f64 == RESTORE_CODE).collect();

    let mut notes: Vec<String> = Vec::new();
    let mut present: Vec<i64> = code
        .iter()
        .zip(&valid)
        .filter(|(_, v)| **v)
        .map(|(c, _)| *c)
        .collect();
    present.sort_unstable();
    present.dedup();
    let unmapped: Vec<i64> = present
        .into_iter()
        .filter(|c| lookup(&MSPA_CLASS_CODES, *c).is_none() && !MSPA_BACKGROUND_CODES.contains(c))
        .collect();
    if !unmapped.is_empty() {
        let unm_ha = code
            .iter()
            .zip(&valid)
            .filter(|(c, v)| **v && unmapped.contains(c))
            .count() as f64
            * pix;
        let list = unmapped
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        notes.push(format!(
            "5.7: the MSPA raster carries code(s) [{list}] ({}) not in MSPA_CLASS_CODES. Verify the GuidosToolbox \
             legend and add them; connector/core areas may be understated until then.",
            fmt_ha(unm_ha)
        ));
    }
    let eco_codes = ints(&ecosystem.values, &ecosystem.mask);
    let eco_word = |area_mask: &[bool]| -> String {
        let mut counts: Vec<(i64, usize)> = Vec::new();
        for (c, _) in eco_codes.iter().zip(area_mask).filter(|(_, m)| **m) {
            match counts.iter_mut().find(|(k, _)| k == c) {
                Some(e) => e.1 += 1,
                None => counts.push((*c, 1)),
            }
        }
        counts.sort_by_key(|(c, _)| *c); // np.unique order
        let mut ha: Vec<(i64, f64)> = counts
            .into_iter()
            .filter(|(c, _)| lookup(&CONNECTIVITY_ECO_WORDS, *c).is_some())
            .map(|(c, n)| (c, n as f64 * pix))
            .collect();
        ha.sort_by(|a, b| b.1.total_cmp(&a.1)); // sorted(..., reverse=True), stable
        let words: Vec<String> = ha
            .iter()
            .filter_map(|(c, _)| lookup(&CONNECTIVITY_ECO_WORDS, *c))
            .map(str::to_string)
            .collect();
        let w = oxford_join(&words);
        if w.is_empty() { "natural".into() } else { w }
    };

    let mut values = serde_json::Map::new();
    let mut sections: Vec<(&str, String)> = Vec::new();
    let protect_hab = and(&protect, &habitat);
    if protect_hab.iter().any(|x| *x) {
        let block_ha = count(&protect_hab) as f64 * pix;
        let corridor_ha = count(&and(&protect, &connector)) as f64 * pix;
        let corridor_pct = safe_pct(corridor_ha, block_ha);
        let word = eco_word(&protect_hab);
        let sentence = format!(
            "Conserving this {word} ecosystem keeps an estimated {} block of natural habitat intact and safeguards \
             its role as a connector in the surrounding landscape. Based on the site structure, {} ({}) of the area \
             functions as a corridor linking separate habitat cores. Protecting it prevents the fragmentation that \
             would likely occur under current deforestation pressure over the {duration_years} year project period.",
            fmt_ha(block_ha),
            fmt_ha(corridor_ha),
            fmt_pct(corridor_pct, 0)
        );
        values.insert(
            "protect".into(),
            json!({"block_ha": block_ha, "corridor_ha": corridor_ha, "corridor_pct": corridor_pct, "ecosystem": word, "narrative": sentence}),
        );
        sections.push(("PROTECT", sentence));
    }
    let manage_hab = and(&manage, &habitat);
    if manage_hab.iter().any(|x| *x) {
        let ce: Vec<bool> = (0..manage.len())
            .map(|i| manage[i] && (connector[i] || edge[i]))
            .collect();
        let conn_edge_ha = count(&ce) as f64 * pix;
        let word = eco_word(&manage_hab);
        let sentence = format!(
            "Managing this {word} ecosystem safeguards and strengthens the physical links between habitat patches \
             across the landscape. The site includes {} of connector and edge habitat whose condition, if improved, \
             supports continued species movement and genetic exchange between nearby cores.",
            fmt_ha(conn_edge_ha)
        );
        values.insert(
            "manage".into(),
            json!({"connector_edge_ha": conn_edge_ha, "ecosystem": word, "narrative": sentence}),
        );
        sections.push(("MANAGE", sentence));
    }
    if restore.iter().any(|x| *x) {
        let n_patches = count_habitat_patches(&core, mspa.width, mspa.height);
        let word = eco_word(&restore);
        let sentence = format!(
            "Restoring this {word} ecosystem re-establishes physical links between {n_patches} separate habitat \
             patches that are currently fragmented, improving the potential for species movement over the \
             {duration_years} year project period."
        );
        values.insert(
            "restore".into(),
            json!({"patch_count": n_patches, "ecosystem": word, "narrative": sentence}),
        );
        sections.push(("RESTORE", sentence));
    }
    if sections.is_empty() {
        return Ok(na(
            "No Protect, Manage or Restore area with mapped habitat is present, so no connectivity benefit can be \
             reported.",
        ));
    }
    let habitat_ha_total = count(&habitat) as f64 * pix;
    let mut class_rows: Vec<(f64, Value)> = Vec::new();
    for (c, name) in MSPA_CLASS_CODES {
        let area = code
            .iter()
            .zip(&valid)
            .filter(|(k, v)| **v && **k == c)
            .count() as f64
            * pix;
        if area > 0.0 {
            let rounded = round_ndigits(area, 1);
            let pct = round_ndigits(safe_pct(area, habitat_ha_total), 1);
            class_rows.push((
                rounded,
                json!({"mspa_class": name, "code": c, "area_ha": rounded, "pct_of_habitat": pct}),
            ));
        }
    }
    class_rows.sort_by(|a, b| b.0.total_cmp(&a.0)); // sorted(key=-area_ha), stable
    let class_rows: Vec<Value> = class_rows.into_iter().map(|(_, v)| v).collect();
    let reported: Vec<&str> = ["protect", "manage", "restore"]
        .into_iter()
        .filter(|k| values.contains_key(*k))
        .collect();
    values.insert(
        "connector_ha_total".into(),
        json!(count(&connector) as f64 * pix),
    );
    values.insert("core_ha_total".into(), json!(count(&core) as f64 * pix));
    values.insert("habitat_ha_total".into(), json!(habitat_ha_total));
    values.insert("pathways_reported".into(), json!(reported));
    values.insert("mspa_year".into(), json!(2024));
    let narratives: serde_json::Map<String, Value> = sections
        .iter()
        .map(|(l, s)| (l.to_string(), json!(s)))
        .collect();
    values.insert("narratives".into(), Value::Object(narratives));
    let narrative = sections
        .iter()
        .map(|(l, s)| format!("{l}: {s}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "pathways_reported",
        "habitat_ha_total",
        "core_ha_total",
        "connector_ha_total",
        "mspa_year",
        "protect",
        "manage",
        "restore",
    ] {
        if let Some(v) = values.get(k) {
            view[k] = v.clone();
        }
    }
    view["mspa_classes"] = json!(class_rows); // after the values, as the dict literal
    if !notes.is_empty() {
        view["notes"] = json!(notes);
    }
    let results = json!({"narrative": narrative, "tables": {"mspa_classes": class_rows}, "values": values, "flags": [], "notes": notes});
    Ok((results, view))
}
