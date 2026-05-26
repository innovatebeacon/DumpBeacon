# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Changed
- **Pivot to Free Forever Model:** DumpBeacon is now completely free to use for all users.
- **Removed Trial Limitations:** The 30-day trial expiration logic has been completely removed from both the Tauri backend and the frontend GUI.
- **Removed License System:** Users are no longer required to enter a license key. The 'Pro' and 'Team' pricing tiers have been removed.
- **Updated License:** Transitioned the project's license from the Business Source License (BSL) 1.1 to the open-source MIT License.

### Removed
- Removed the license verification UI (modals, inputs, and buttons) from the web preview.
- Removed the Lemon Squeezy payment integration from the frontend.
- Removed backend checks that read `beacon.cfg` to validate trial status.
