# Spec Delta

## Purpose

Defines the Windows 11 style user interface of Linux Office Installer Suite: navigation, pages, theming and the visual rules. The main one is that no gradient appears anywhere.

## ADDED Requirements

### Requirement: Application shell
The window SHALL have a left navigation pane with these entries:
- Install
- Progress & Log
- Office apps
- Wine runtimes
- Settings, pinned at the bottom

The selected entry SHALL be marked with an accent-colored indicator. The pane SHALL collapse to icons only when the window is narrower than 1008 px.

#### Scenario: Navigate pages
- **WHEN** the user clicks "Wine runtimes" in the navigation pane
- **THEN** the Wine runtimes page is shown and its nav entry shows the accent indicator

#### Scenario: Narrow window
- **WHEN** the window width drops below 1008 px
- **THEN** the navigation pane shows icons only, and labels appear as tooltips

### Requirement: Install page
The Install page SHALL show a dependency status card and the deployment form, grouped into cards: Product & release, Apps, Language, and a collapsed Advanced expander. Footer actions SHALL be "Show XML" and a primary "Install" button. "Install" SHALL be disabled while an installation is running.

#### Scenario: Missing dependencies
- **WHEN** required dependencies are missing
- **THEN** the dependency card shows a warning banner with an "Install packages" action, and "Install" remains enabled but its first stage resolves dependencies

### Requirement: Progress & Log page
The Progress & Log page SHALL show:
- the pipeline stages with their status
- the current stage's progress bar
- a Cancel button
- a collapsible log panel showing live output
- a result banner (success or error) when the run ends

#### Scenario: Run finishes
- **WHEN** the install pipeline completes
- **THEN** a success banner appears with actions to open the Office apps page

### Requirement: Office apps and Wine runtimes pages
The Office apps page and the Wine runtimes page SHALL each list their items as rows with a name, version or status, and an overflow ("…") menu. Destructive actions in those menus SHALL require a confirmation dialog.
- Office apps rows: Launch, Recreate launcher.
- Office apps page action: Uninstall Office.
- Wine runtimes rows: Set as active, Remove.
- Wine runtimes page action: Install new version.

#### Scenario: Confirm removal
- **WHEN** the user chooses Remove on a runner
- **THEN** a dialog asks for confirmation, with the safe "Cancel" button as default

### Requirement: No gradients
No UI element SHALL be painted with a linear, radial or conic gradient. This covers backgrounds, borders, buttons, inputs, progress indicators and icons. All fills and strokes SHALL be solid colors.

#### Scenario: Visual audit
- **WHEN** the UI source is searched for gradient constructs
- **THEN** none are found, and every rendered control uses only solid fills and strokes

### Requirement: Windows 11 visual language
The UI SHALL follow these visual rules:
- Window background: solid `#F3F3F3` in light theme, `#202020` in dark.
- Content is grouped on solid cards with a 1 px solid stroke and 4 px corner radius. Dialogs use an 8 px radius.
- Accent: `#005FB8` in light theme, `#60CDFF` in dark.
- Typography uses the bundled Inter font (SIL OFL) at these sizes: Caption 12, Body 14, Body Strong 14 semibold, Subtitle 20 semibold, Title 28 semibold. The application SHALL NOT bundle Microsoft-authored fonts.

#### Scenario: Card appearance
- **WHEN** the Install page is shown in light theme
- **THEN** settings appear on solid light cards with a 1 px border and 4 px rounded corners over the `#F3F3F3` background

### Requirement: Light and dark themes
The UI SHALL follow the system color scheme by default. Settings SHALL allow overriding it with Light or Dark, and the choice is saved. Switching themes SHALL apply immediately, without a restart.

#### Scenario: Override to dark
- **WHEN** the user selects Dark in Settings
- **THEN** the whole UI switches to the dark palette immediately and stays dark after restart

### Requirement: Application icon
The application SHALL use an original, flat, gradient-free icon that contains no Microsoft logos or trademarks. It SHALL be provided at the standard hicolor sizes (16, 32, 48, 64, 128, 256, 512 px) and used for the window and the app's desktop entry.

#### Scenario: Menu icon
- **WHEN** the app is self-installed
- **THEN** its menu entry shows the original app icon at the appropriate size
