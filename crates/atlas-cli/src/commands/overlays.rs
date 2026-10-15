use atlas_project::{AtlasHome, Binding, OverlayListing, OverlayState, Overlays, elapsed_text};
use serde_json::{Value, json};

use crate::output::note;

pub(crate) struct OverlayReport {
  pub lines: Vec<String>,
  pub json: Option<(&'static str, Value)>,
}

pub(crate) fn listings(home: &AtlasHome, base: &str) -> Vec<OverlayListing> {
  match Overlays::new(home).list() {
    Ok(listings) => listings.into_iter().filter(|listing| listing.record.base.as_str() == base).collect(),
    Err(error) => {
      note(format!("warning: cannot list worktree overlays: {error}"));
      Vec::new()
    }
  }
}

pub(crate) fn report(home: &AtlasHome, binding: &Binding, project: &str) -> OverlayReport {
  if let Binding::Overlay(overlay) = binding {
    let report = overlay.report(home);
    return OverlayReport {
      lines: vec![report.line],
      json: Some(("overlay", report.json)),
    };
  }
  let listings = listings(home, project);
  if listings.is_empty() {
    return OverlayReport { lines: Vec::new(), json: None };
  }
  OverlayReport {
    lines: listings.iter().map(|listing| format!("worktree overlay {}", listing_line(listing))).collect(),
    json: Some(("overlays", Value::Array(listings.iter().map(listing_json).collect()))),
  }
}

pub(crate) fn listing_line(listing: &OverlayListing) -> String {
  format!(
    "{}  {}  ({}, last used {} ago)",
    listing.record.label,
    listing.record.worktree.display(),
    state_name(listing.state),
    elapsed_text(listing.record.last_used_ms)
  )
}

fn listing_json(listing: &OverlayListing) -> Value {
  json!({
    "label": listing.record.label,
    "name": listing.record.name.as_str(),
    "worktree": listing.record.worktree.display().to_string(),
    "state": state_name(listing.state),
    "last_used_ms": listing.record.last_used_ms,
    "seeded_ms": listing.record.seeded_ms,
  })
}

fn state_name(state: OverlayState) -> &'static str {
  match state {
    OverlayState::Ready => "ready",
    OverlayState::NotSeeded => "not seeded",
    OverlayState::WorktreeGone => "worktree gone",
  }
}
