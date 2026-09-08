# AccessKit AT-SPI disabled state repair

Source: crates.io accesskit_atspi_common 0.18.1, original package SHA-256 `1e8c61bee90b42a772d39d06a740207dc71a4e780004ace1db8d99fb1baaa954`. Vendored source and normalized manifest retain upstream notices. This patch changes only Enabled/Sensitive emission for disabled controls: read-only-unsupported roles such as Button must not report enabled when is_disabled is true.

The override preserves the upstream version and dependency graph. Remove it when upgrading to an upstream adapter with this repair. Native verification uses Browser Back/Forward buttons before history exists and active Reload.
