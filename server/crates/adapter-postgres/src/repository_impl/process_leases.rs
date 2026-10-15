use super::*;
use sideseat_ports::traits::{ProcessLeases, ProcessRole};

use super::super::repositories::process_lease;

#[async_trait]
impl ProcessLeases for PostgresRepository {
    async fn renew_process_lease(
        &self,
        process_id: &str,
        role: ProcessRole,
        lease_secs: i64,
    ) -> Result<(), DataError> {
        let role = role.as_str();
        self.0
            .lease_connection()
            .run(|connection| {
                let id = process_id.to_owned();
                Box::pin(
                    async move { process_lease::renew(connection, &id, role, lease_secs).await },
                )
            })
            .await
            .map_err(Into::into)
    }

    async fn end_process_lease(&self, process_id: &str) -> Result<(), DataError> {
        self.0
            .lease_connection()
            .run(|connection| {
                let id = process_id.to_owned();
                Box::pin(async move { process_lease::end(connection, &id).await })
            })
            .await
            .map_err(Into::into)
    }

    async fn live_process_leases(
        &self,
        role: ProcessRole,
        except: &str,
    ) -> Result<Vec<String>, DataError> {
        let role = role.as_str();
        self.0
            .lease_connection()
            .run(|connection| {
                let except = except.to_owned();
                Box::pin(async move { process_lease::live(connection, role, &except).await })
            })
            .await
            .map_err(Into::into)
    }
}
