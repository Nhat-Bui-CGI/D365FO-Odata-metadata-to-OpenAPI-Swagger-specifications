//! Which Dynamics 365 OData service a document describes.

use crate::edm::Metadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Platform {
    /// Finance & Operations: `https://<env>.operations.dynamics.com/data`.
    #[default]
    FinanceOperations,
    /// Dataverse (Customer Engagement, Sales, Field Service, Power Apps):
    /// `https://<org>.crm<n>.dynamics.com/api/data/v9.2`.
    Dataverse,
}

impl Platform {
    /// Guesses the platform from an environment URL. Dataverse hosts have a
    /// `crm`, `crm4`, `crm11`… label (also in the government clouds).
    pub fn detect(url: &str) -> Self {
        let host = url.split("://").last().unwrap_or(url);
        let host = host.split(['/', ':']).next().unwrap_or(host);
        let is_crm_label = |label: &str| {
            label
                .to_ascii_lowercase()
                .strip_prefix("crm")
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        };
        if host.split('.').any(is_crm_label) {
            Self::Dataverse
        } else {
            Self::FinanceOperations
        }
    }

    /// Guesses the platform from the metadata itself.
    pub fn of_metadata(md: &Metadata) -> Self {
        if md
            .entity_types
            .iter()
            .any(|t| t.namespace == "Microsoft.Dynamics.CRM")
        {
            Self::Dataverse
        } else {
            Self::FinanceOperations
        }
    }

    /// Path of the OData service root, without a trailing slash.
    pub fn data_path(self) -> &'static str {
        match self {
            Self::FinanceOperations => "/data",
            Self::Dataverse => "/api/data/v9.2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FinanceOperations => "Finance & Operations",
            Self::Dataverse => "Dataverse",
        }
    }

    /// Short name for UI text, e.g. "D365FO" or "Dataverse".
    pub fn short_name(self) -> &'static str {
        match self {
            Self::FinanceOperations => "D365FO",
            Self::Dataverse => "Dataverse",
        }
    }

    pub(crate) fn example_host(self) -> &'static str {
        match self {
            Self::FinanceOperations => "your-environment.operations.dynamics.com",
            Self::Dataverse => "your-org.crm.dynamics.com",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_from_url() {
        for (url, p) in [
            (
                "https://contoso.operations.dynamics.com",
                Platform::FinanceOperations,
            ),
            (
                "https://contoso-uat.sandbox.operations.dynamics.com/",
                Platform::FinanceOperations,
            ),
            ("https://contoso.crm.dynamics.com", Platform::Dataverse),
            (
                "https://contoso.crm4.dynamics.com/api/data/v9.2",
                Platform::Dataverse,
            ),
            ("contoso.CRM11.dynamics.com", Platform::Dataverse),
            ("https://contoso.crm9.dynamics.com", Platform::Dataverse),
            (
                "https://crmdemo.operations.dynamics.com",
                Platform::FinanceOperations,
            ),
        ] {
            assert_eq!(Platform::detect(url), p, "{url}");
        }
    }
}
