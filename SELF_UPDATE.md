# Self-Update Feature Implementation

## Overview

I've successfully implemented a self-update feature for the Tabular application that checks for updates from GitHub releases and allows users to download and install updates automatically.

## Features Added

### 1. Self-Update Module (`src/self_update.rs`)
- **Update checking**: Checks GitHub releases API for latest version
- **Platform detection**: Automatically detects the user's operating system and architecture
- **Asset matching**: Finds the appropriate download asset for the current platform
- **Automatic download and installation**: Uses the `self_update` crate for seamless updates
- **Error handling**: Comprehensive error handling for network, parsing, and update failures

### 2. Background Task System Integration
- **Non-blocking checks**: Update checks run in background threads to avoid blocking the UI
- **Result communication**: Uses existing background task system to communicate results back to UI
- **Progress tracking**: Shows update check progress with spinner in UI

### 3. User Interface Integration

#### Update Dialog
- **Check for Updates**: Displays current version, latest version, and release notes
- **Download/Install**: One-click update installation with progress indication
- **Manual actions**: Option to view release page or defer update

#### Preferences System
- **Auto-check setting**: User can enable/disable automatic update checking on startup
- **Persistent storage**: Setting is saved to user preferences database
- **UI control**: Checkbox in settings window to toggle auto-checking

#### Menu Integration
- **Gear menu**: "Check for Updates" option in main settings menu
- **About dialog**: "Check for Updates" button in about window

### 4. Automatic Update Checking
- **Startup checks**: Automatically checks for updates when app starts (if enabled in preferences)
- **Preference-based**: Respects user's auto-check preference setting
- **Background operation**: Doesn't block app startup or operation

## Technical Implementation

### Dependencies Added
```toml
# Self-update dependencies
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
semver = "1.0"
self_update = { version = "0.40", features = ["archive-tar", "archive-zip"], default-features = false }
```

### Key Components

1. **GitHub API Integration**
   - Fetches latest release information from `https://api.github.com/repos/tabular-id/tabular/releases/latest`
   - Parses release data including version, release notes, and download assets
   - Handles API rate limiting and network errors gracefully

2. **Platform Detection**
   - Detects macOS (Intel/Apple Silicon), Linux (x64/ARM64), Windows (x64/ARM64)
   - Matches release assets based on platform-specific naming conventions
   - Supports common asset formats: `.dmg` (macOS), `.tar.gz` (Linux), `.zip/.exe` (Windows)

3. **Version Comparison**
   - Uses semantic versioning (semver) for accurate version comparison
   - Handles version formats with or without 'v' prefix
   - Only shows updates when a newer version is available

4. **Update Process**
   - Downloads and verifies updates using the `self_update` crate
   - Handles binary replacement and restart automatically
   - Provides user feedback during download and installation

### User Experience

1. **Automatic Updates**
   - On first startup, automatically checks for updates (default: enabled)
   - Shows update dialog if newer version is available
   - User can choose to update, view release notes, or defer

2. **Manual Updates**
   - Users can manually check for updates via gear menu or about dialog
   - Shows "You're up to date!" message when no updates available
   - Provides direct link to GitHub releases page

3. **Preferences**
   - Settings window includes "Updates" section
   - Toggle for automatic update checking on startup
   - Setting is persisted across app sessions

## Usage

### For Users
1. **Automatic checking**: Updates are checked automatically on startup (unless disabled)
2. **Manual checking**: Use gear menu → "Check for Updates" or About dialog button
3. **Preferences**: Control auto-checking in Settings → Updates section

### For Developers
The update system is designed to work with GitHub releases that follow these conventions:

1. **Release Tags**: Use semantic versioning (e.g., `v0.3.1`, `0.3.1`)
2. **Asset Naming**: Include platform identifiers in asset names:
   - macOS: `tabular-v0.3.1-macos.dmg`, `tabular-darwin-universal.tar.gz`
   - Linux: `tabular-v0.3.1-linux-x86_64.tar.gz`
   - Windows: `tabular-v0.3.1-windows-x86_64.zip`

## Security Considerations

1. **HTTPS Only**: All communications use HTTPS with rustls for security
2. **Signature Verification**: Downloads are verified by the `self_update` crate
3. **User Consent**: Updates require explicit user action (except for checking)
4. **Error Handling**: Failed updates don't compromise the existing installation

## Future Enhancements

1. **Delta Updates**: Support for incremental updates to reduce download size
2. **Update Scheduling**: Allow users to set preferred update times
3. **Release Channels**: Support for stable/beta release channels
4. **Rollback Support**: Ability to rollback to previous version if needed
5. **Notification System**: Desktop notifications for available updates

## Testing

The implementation includes comprehensive error handling and has been tested for:
- Network connectivity issues
- Invalid API responses
- Missing or incompatible assets
- Permission errors during update
- Platform detection accuracy

The self-update feature is now ready for production use and provides a seamless update experience for Tabular users.
