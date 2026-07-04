#![forbid(unsafe_code)]

#[cfg(feature = "hera-dogfood")]
include!(concat!(env!("OUT_DIR"), "/hera_render_model.rs"));

#[cfg(not(feature = "hera-dogfood"))]
pub struct DogfoodFeatureDisabled;
