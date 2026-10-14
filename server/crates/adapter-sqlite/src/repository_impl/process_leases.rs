use super::*;
use sideseat_ports::traits::{ProcessLeases, ProcessRole};

use super::super::repositories::process_lease;

#[async_trait]
impl ProcessLeases for SqliteRepository {
    async fn renew_process_lease(
        &self,
        process_id: &str,
        role: ProcessRole,
        lease_secs: i64,
    ) -> Result<(), DataError> {
        process_lease::renew(self.0.pool(), process_id, role.as_str(), lease_secs)
            .await
            .map_err(Into::into)
    }

    async fn end_process_lease(&self, process_id: &str) -> Result<(), DataError> {
        process_lease::end(self.0.pool(), process_id)
            .await
            .map_err(Into::into)
    }

    async fn live_process_leases(
        &self,
        role: ProcessRole,
        except: &str,
    ) -> Result<Vec<String>, DataError> {
        process_lease::live(self.0.pool(), role.as_str(), except)
            .await
            .map_err(Into::into)
    }
}
