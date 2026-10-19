//! The product picker: plan names users recognise, mapped to ODT Product IDs (design D8).
//! Only Retail products are listed; volume-licensed editions are never offered.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Plan {
    #[default]
    M365Family,
    M365Enterprise,
    M365AppsEnterpriseNoTeams,
    M365Business,
    M365AppsBusinessNoTeams,
    Home2024,
    HomeBusiness2024,
    Professional2024,
    ProPlus2024,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Channel {
    #[default]
    Current,
    MonthlyEnterprise,
    SemiAnnual,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Current, Channel::MonthlyEnterprise, Channel::SemiAnnual];

    pub fn odt_name(self) -> &'static str {
        match self {
            Channel::Current => "Current",
            Channel::MonthlyEnterprise => "MonthlyEnterprise",
            Channel::SemiAnnual => "SemiAnnual",
        }
    }
}

impl Plan {
    pub const ALL: [Plan; 9] = [
        Plan::M365Family,
        Plan::M365Enterprise,
        Plan::M365AppsEnterpriseNoTeams,
        Plan::M365Business,
        Plan::M365AppsBusinessNoTeams,
        Plan::Home2024,
        Plan::HomeBusiness2024,
        Plan::Professional2024,
        Plan::ProPlus2024,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Plan::M365Family => "Microsoft 365 Family / Personal",
            Plan::M365Enterprise => "Microsoft 365 E3/E5, Office 365 E3/E5",
            Plan::M365AppsEnterpriseNoTeams => "Microsoft 365 Apps for enterprise, E3/E5 (no Teams)",
            Plan::M365Business => "Microsoft 365 Business Standard / Premium",
            Plan::M365AppsBusinessNoTeams => "Microsoft 365 Apps for business, Business (no Teams)",
            Plan::Home2024 => "Office Home 2024",
            Plan::HomeBusiness2024 => "Office Home & Business 2024",
            Plan::Professional2024 => "Office Professional 2024",
            Plan::ProPlus2024 => "Office Professional Plus 2024",
        }
    }

    pub fn product_id(self) -> &'static str {
        match self {
            Plan::M365Family => "O365HomePremRetail",
            Plan::M365Enterprise => "O365ProPlusRetail",
            Plan::M365AppsEnterpriseNoTeams => "O365ProPlusEEANoTeamsRetail",
            Plan::M365Business => "O365BusinessRetail",
            Plan::M365AppsBusinessNoTeams => "O365BusinessEEANoTeamsRetail",
            Plan::Home2024 => "Home2024Retail",
            Plan::HomeBusiness2024 => "HomeBusiness2024Retail",
            Plan::Professional2024 => "Professional2024Retail",
            Plan::ProPlus2024 => "ProPlus2024Retail",
        }
    }

    pub fn is_retail_2024(self) -> bool {
        matches!(self, Plan::Home2024 | Plan::HomeBusiness2024 | Plan::Professional2024 | Plan::ProPlus2024)
    }

    /// Channels the user may pick. 2024 Retail is locked to Current.
    pub fn allowed_channels(self) -> &'static [Channel] {
        if self.is_retail_2024() { &[Channel::Current] } else { &Channel::ALL }
    }

    pub fn channel_editable(self) -> bool {
        self.allowed_channels().len() > 1
    }

    /// The channel that is actually used for this plan.
    pub fn effective_channel(self, chosen: Channel) -> Channel {
        if self.allowed_channels().contains(&chosen) { chosen } else { Channel::Current }
    }
}

pub const PLAN_WARNING: &str =
    "Pick the plan your Microsoft account actually holds. Installing a different plan prevents activation.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_ids_match_the_spec_table() {
        let expected = [
            (Plan::M365Family, "O365HomePremRetail"),
            (Plan::M365Enterprise, "O365ProPlusRetail"),
            (Plan::M365AppsEnterpriseNoTeams, "O365ProPlusEEANoTeamsRetail"),
            (Plan::M365Business, "O365BusinessRetail"),
            (Plan::M365AppsBusinessNoTeams, "O365BusinessEEANoTeamsRetail"),
            (Plan::Home2024, "Home2024Retail"),
            (Plan::HomeBusiness2024, "HomeBusiness2024Retail"),
            (Plan::Professional2024, "Professional2024Retail"),
            (Plan::ProPlus2024, "ProPlus2024Retail"),
        ];
        assert_eq!(expected.len(), Plan::ALL.len());
        for (plan, id) in expected {
            assert_eq!(plan.product_id(), id, "{plan:?}");
            assert!(id.ends_with("Retail"), "only retail products may be offered");
        }
        assert_eq!(Plan::default(), Plan::M365Family);
    }

    #[test]
    fn retail_2024_locks_channel_to_current() {
        for plan in Plan::ALL {
            if plan.is_retail_2024() {
                assert_eq!(plan.allowed_channels(), &[Channel::Current]);
                assert!(!plan.channel_editable());
                assert_eq!(plan.effective_channel(Channel::SemiAnnual), Channel::Current);
            } else {
                assert_eq!(plan.allowed_channels(), &Channel::ALL);
                assert!(plan.channel_editable());
                assert_eq!(plan.effective_channel(Channel::SemiAnnual), Channel::SemiAnnual);
            }
        }
    }
}
