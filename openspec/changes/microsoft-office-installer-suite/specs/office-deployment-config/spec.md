# Spec Delta

## Purpose

Turns the user's choices in a simple form into a valid Office Deployment Tool `Configuration.xml`, with safe defaults that work under Wine.

## ADDED Requirements

### Requirement: Product choices
The product picker SHALL list the plan names the user recognises, each mapped to the Product ID Microsoft documents for that plan:

| Plan shown to the user | Product ID |
| --- | --- |
| Microsoft 365 Family / Personal | `O365HomePremRetail` |
| Microsoft 365 E3/E5, Office 365 E3/E5 | `O365ProPlusRetail` |
| Microsoft 365 Apps for enterprise, E3/E5 (no Teams) | `O365ProPlusEEANoTeamsRetail` |
| Microsoft 365 Business Standard / Premium | `O365BusinessRetail` |
| Microsoft 365 Apps for business, Business (no Teams) | `O365BusinessEEANoTeamsRetail` |
| Office Home 2024 | `Home2024Retail` |
| Office Home & Business 2024 | `HomeBusiness2024Retail` |
| Office Professional 2024 | `Professional2024Retail` |
| Office Professional Plus 2024 | `ProPlus2024Retail` |

The default SHALL be Microsoft 365 Family / Personal. The picker SHALL warn that choosing a plan the account doesn't hold prevents activation. Volume-licensed products SHALL NOT be offered.

#### Scenario: Default product
- **WHEN** the user opens the Install page for the first time
- **THEN** Microsoft 365 Family / Personal is selected

#### Scenario: Plan maps to Product ID
- **WHEN** the user selects "Microsoft 365 Apps for business, Business (no Teams)"
- **THEN** the generated XML contains `<Product ID="O365BusinessEEANoTeamsRetail">`

### Requirement: Architecture and channel
The form SHALL offer 64-bit (default) and 32-bit architectures. For Microsoft 365 products, the update channel choices SHALL be `Current` (default), `MonthlyEnterprise` and `SemiAnnual`. For 2024 Retail products, the channel SHALL be `Current` and not editable.

#### Scenario: 2024 product locks channel
- **WHEN** the user selects Office Home 2024
- **THEN** the channel control shows `Current` and is disabled

#### Scenario: Architecture mismatch with existing install
- **WHEN** Office is already installed in the prefix with a different architecture than the one selected
- **THEN** the system blocks generation and explains that mixed architectures are not supported

### Requirement: App selection
The form SHALL list Word, Excel, PowerPoint, Outlook, OneNote, Access, Publisher, OneDrive, Teams and Skype for Business, each as a checkbox. Unchecked apps SHALL become `<ExcludeApp>` elements. OneDrive SHALL map to the ID `Groove` and Skype for Business to `Lync`. By default Teams, OneDrive (Groove) and Skype for Business (Lync) SHALL be unchecked. Apps with known Wine issues (Outlook, PowerPoint, OneNote) SHALL show a "known issues" note.

#### Scenario: Excluding OneDrive
- **WHEN** the user leaves OneDrive unchecked
- **THEN** the generated XML contains `<ExcludeApp ID="Groove" />` under the product

### Requirement: Languages
The language setting SHALL default to `MatchOS` with fallback `en-us`. The user MAY instead pick one or more specific language tags. When `MatchOS` is used, `en-us` SHALL also be included as an explicit language. That avoids missing locale resources for unsupported system locales.

#### Scenario: Default languages
- **WHEN** the user keeps the default language setting
- **THEN** the XML contains `<Language ID="MatchOS" Fallback="en-us" />` and `<Language ID="en-us" />`

### Requirement: Advanced options
An Advanced section SHALL expose these options:
- Updates enabled: default off, producing `<Updates Enabled="FALSE" />`.
- Display level: `None` (default) or `Full`.
- Logging: on by default, written to a fixed log folder inside the prefix.

`AcceptEULA="TRUE"` SHALL always be set. No license key, KMS or activation-bypass settings SHALL ever be generated.

#### Scenario: Updates default
- **WHEN** the user does not open Advanced
- **THEN** the XML contains `<Updates Enabled="FALSE" />`

#### Scenario: No license data
- **WHEN** any configuration is generated
- **THEN** the XML contains no `PIDKEY`, `AUTOACTIVATE` or KMS-related properties

### Requirement: Generated file and preview
The system SHALL write the generated configuration to `~/.msoffice/Configuration.xml` and to the prefix's `C:\` drive. The configuration SHALL be well-formed XML. The Install page SHALL offer "Show XML" to preview the exact content before installing, and "Save as…" to export it.

#### Scenario: Preview matches installed config
- **WHEN** the user opens "Show XML" and then installs without changing the form
- **THEN** the file passed to setup is byte-identical to the previewed content
