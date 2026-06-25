//! ETHERA: sidecar mailbox + shared-publisher relay queries on [`DriverDBClient`].

use sqlx::{postgres::PgQueryResult, Error, Row};

use crate::{CommitmentConfig, DriverDBClient, RequestStatus, RequestType};

impl DriverDBClient {
    /// Update the mailbox store fields for a request.
    pub async fn update_mailbox_store(
        &self,
        id: i64,
        inbox_chains: Option<Vec<Vec<u8>>>,
        outbox_chains: Option<Vec<Vec<u8>>>,
        inbox_roots: Option<Vec<Vec<u8>>>,
        outbox_roots: Option<Vec<Vec<u8>>>,
        mailbox_root: Option<Vec<u8>>,
    ) -> Result<PgQueryResult, Error> {
        sqlx::query(
            r#"
            UPDATE requests SET
                mailbox_inbox_chains = $1,
                mailbox_outbox_chains = $2,
                mailbox_inbox_roots = $3,
                mailbox_outbox_roots = $4,
                mailbox_root = $5,
                updated_at = NOW()
            WHERE id = $6
            "#,
        )
        .bind(inbox_chains.as_ref().map(|v| v.as_slice()))
        .bind(outbox_chains.as_ref().map(|v| v.as_slice()))
        .bind(inbox_roots.as_ref().map(|v| v.as_slice()))
        .bind(outbox_roots.as_ref().map(|v| v.as_slice()))
        .bind(mailbox_root.as_ref().map(|v| v.as_slice()))
        .bind(id)
        .execute(&self.pool)
        .await
    }

    /// Fetch mailbox store data for a matching request.
    pub async fn fetch_mailbox_store(
        &self,
        end_block: i64,
        l1_chain_id: i64,
        l2_chain_id: i64,
        req_type: RequestType,
        commitment: &CommitmentConfig,
    ) -> Result<
        Option<(
            Option<Vec<Vec<u8>>>,
            Option<Vec<Vec<u8>>>,
            Option<Vec<Vec<u8>>>,
            Option<Vec<Vec<u8>>>,
            Option<Vec<u8>>,
        )>,
        Error,
    > {
        let result = sqlx::query(
            r#"
            SELECT
                mailbox_inbox_chains,
                mailbox_outbox_chains,
                mailbox_inbox_roots,
                mailbox_outbox_roots,
                mailbox_root
            FROM requests
            WHERE end_block = $1
                AND l1_chain_id = $2
                AND l2_chain_id = $3
                AND req_type = $4
                AND range_vkey_commitment = $5
                AND rollup_config_hash = $6
            ORDER BY id DESC
            LIMIT 1
            "#,
        )
        .bind(end_block)
        .bind(l1_chain_id)
        .bind(l2_chain_id)
        .bind(req_type as i16)
        .bind(&commitment.range_vkey_commitment[..])
        .bind(&commitment.rollup_config_hash[..])
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = result else {
            return Ok(None);
        };

        Ok(Some((
            row.try_get("mailbox_inbox_chains")?,
            row.try_get("mailbox_outbox_chains")?,
            row.try_get("mailbox_inbox_roots")?,
            row.try_get("mailbox_outbox_roots")?,
            row.try_get("mailbox_root")?,
        )))
    }

    /// Get the latest relayed aggregation block number for a specific chain pair.
    ///
    /// Multiple validity proposers share the same database, so this query must be scoped by chain
    /// IDs to avoid one rollup advancing another rollup's relay baseline.
    pub async fn get_latest_relayed_block_number(
        &self,
        l1_chain_id: i64,
        l2_chain_id: i64,
    ) -> Result<Option<i64>, Error> {
        let latest_relayed = sqlx::query(
            r#"
            SELECT end_block
            FROM requests
            WHERE req_type = $1 AND status = $2 AND l1_chain_id = $3 AND l2_chain_id = $4
            ORDER BY end_block DESC
            LIMIT 1
            "#,
        )
        .bind(RequestType::Aggregation as i16)
        .bind(RequestStatus::Relayed as i16)
        .bind(l1_chain_id)
        .bind(l2_chain_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(latest_relayed.map(|record| record.get("end_block")))
    }
}
