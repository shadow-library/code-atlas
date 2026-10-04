use atlas_core::{ATTR_IMPL_OF, Entity, is_test_code};

use crate::owner::ATTR_GENERATED;

pub(super) fn is_generated(entity: &Entity) -> bool {
  entity.attrs.get(ATTR_GENERATED).and_then(serde_json::Value::as_bool).unwrap_or(false)
}

pub(super) fn implements_trait(entity: &Entity) -> bool {
  entity
    .attrs
    .get(ATTR_IMPL_OF)
    .and_then(|impl_of| impl_of.get("trait_ref"))
    .is_some_and(serde_json::Value::is_object)
}

pub(super) fn is_production_code(entity: &Entity) -> bool {
  !is_test_code(entity) && !is_generated(entity)
}
