#![forbid(unsafe_code)]

#[cfg(feature = "hera-dogfood")]
include!(concat!(env!("OUT_DIR"), "/hera_protocol.rs"));

#[cfg(not(feature = "hera-dogfood"))]
pub struct DogfoodFeatureDisabled;
