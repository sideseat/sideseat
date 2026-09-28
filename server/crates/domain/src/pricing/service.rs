use super::*;

/// Thread-safe pricing service with background sync
pub struct PricingService {
    /// Pricing data (read-heavy, RwLock for concurrent reads)
    pub(super) data: RwLock<PricingData>,

    /// Path to local pricing file in data directory
    pub(super) local_path: PathBuf,

    pub(super) catalogue_source: Option<Arc<dyn PricingCatalogueSource>>,
    pub(super) clock: Arc<dyn Clock>,
}

impl PricingService {
    /// Initialize pricing service
    ///
    /// Loading priority:
    /// 1. Try local file from data directory
    /// 2. If local valid and has >= models than embedded, use it
    /// 3. Otherwise, use embedded data and save to disk
    ///
    /// An optional catalogue source is retained for a runtime-owned background sync task.
    pub async fn init(
        storage: &AppStorage,
        clock: Arc<dyn Clock>,
        catalogue_source: Option<Arc<dyn PricingCatalogueSource>>,
    ) -> Result<Arc<Self>, PricingError> {
        let local_path = storage.data_dir().join(PRICING_FILE_NAME);

        let data = Self::load_pricing_data(&local_path, clock.as_ref()).await?;

        Ok(Arc::new(Self {
            data: RwLock::new(data),
            local_path,
            catalogue_source,
            clock,
        }))
    }

    /// Load pricing data: the local file when it is the better catalogue, else this build's embedded one.
    ///
    /// "Better" used to mean *larger* - `local.model_count >= embedded_count` - and a model count is not a
    /// statement about freshness. A catalogue that accumulated retired models is bigger and more wrong, so a
    /// stale local file won on size and pinned its prices, and upgrading the binary could not dislodge it.
    ///
    /// What decides it now is **provenance**, recorded when the file is written (see [`PricingProvenance`]):
    ///
    /// The rule is one question, asked of either source: **was this file written by the build that is now
    /// running?** Its `embedded_digest` records the snapshot current when it was written, so:
    ///
    /// - digest matches - the file is either this build's own snapshot, or a sync fetched *over* it, which is
    ///   strictly newer upstream data. Used.
    /// - digest differs, or there is no provenance at all - the binary has been upgraded since, so this
    ///   build's snapshot may hold corrections the file predates. Replaced.
    ///
    /// "A sync always wins" was the first version of this and was wrong in two ways. With `sync_hours = 0`,
    /// or with the network unavailable, a January sync was preferred over a September release's corrected
    /// prices *forever* - the reasoning "the startup sync refreshes it anyway" assumed a sync that may never
    /// run. And it broke agreement between replicas: a long-lived replica priced from its January file while
    /// a freshly-started one priced from September's snapshot, and the cost is persisted at ingestion, so the
    /// same request was stored at two different prices depending on routing. Under this rule every replica of
    /// a given build agrees, and the only divergence left is a replica that has synced since starting - which
    /// is upstream data the others converge on.
    ///
    /// Every branch is decidable from facts this code controls: no timestamps, no counts, no proxies.
    pub(super) async fn load_pricing_data(
        local_path: &Path,
        clock: &dyn Clock,
    ) -> Result<PricingData, PricingError> {
        if !local_path.exists() {
            return Self::load_embedded_with_save(local_path, clock).await;
        }

        match Self::try_load_local(local_path).await {
            Ok(local_data) => {
                let provenance = Self::read_provenance(local_path).await;
                let keep = provenance
                    .as_ref()
                    .is_some_and(|p| p.embedded_digest.as_deref() == Some(embedded_digest()));
                if keep {
                    tracing::debug!(
                        models = local_data.model_count,
                        source = provenance
                            .as_ref()
                            .map(|p| p.source.as_str())
                            .unwrap_or("none"),
                        "Using the local pricing catalogue"
                    );
                    Ok(local_data)
                } else {
                    tracing::debug!(
                        "The local pricing catalogue predates this build; replacing it with this \
                         build's snapshot, which the next sync will update"
                    );
                    Self::load_embedded_with_save(local_path, clock).await
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load local pricing, using embedded");
                Self::load_embedded_with_save(local_path, clock).await
            }
        }
    }

    /// Load embedded pricing data and save to disk (best-effort)
    async fn load_embedded_with_save(
        local_path: &Path,
        clock: &dyn Clock,
    ) -> Result<PricingData, PricingError> {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)?;
        if let Err(e) = Self::save_to_file(local_path, EMBEDDED_PRICING_JSON).await {
            tracing::warn!(error = %e, "Failed to save pricing to disk (continuing with embedded)");
        } else {
            Self::write_provenance(
                local_path,
                PricingProvenance {
                    source: PROVENANCE_EMBEDDED.to_string(),
                    embedded_digest: Some(embedded_digest().to_string()),
                    written_at: clock.now().to_rfc3339(),
                },
            )
            .await;
        }
        Ok(data)
    }

    /// Read the sidecar beside the catalogue. Absent or unreadable is simply "unknown provenance", which
    /// the caller treats as "not this build's" - the safe direction, since the cost is re-saving a file.
    pub(super) async fn read_provenance(local_path: &Path) -> Option<PricingProvenance> {
        let raw = tokio::fs::read_to_string(provenance_path(local_path))
            .await
            .ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Best-effort: a missing sidecar costs one re-save, never a wrong price.
    pub(super) async fn write_provenance(local_path: &Path, provenance: PricingProvenance) {
        let path = provenance_path(local_path);
        match serde_json::to_string(&provenance) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&path, json).await {
                    tracing::debug!(error = %e, "Could not record pricing provenance");
                }
            }
            Err(e) => tracing::debug!(error = %e, "Could not serialise pricing provenance"),
        }
    }

    /// Create PricingService for testing (no file I/O)
    #[cfg(any(test, feature = "test-support"))]
    pub fn init_for_test() -> Result<Self, PricingError> {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)?;
        Ok(Self {
            data: RwLock::new(data),
            local_path: std::env::temp_dir().join("sideseat_test_pricing.json"),
            catalogue_source: None,
            clock: Arc::new(TestClock),
        })
    }

    /// Try to load pricing data from local file
    async fn try_load_local(path: &Path) -> Result<PricingData, PricingError> {
        let json = tokio::fs::read_to_string(path).await?;
        PricingData::from_json_str(&json)
    }

    /// Save pricing data to file atomically (write to temp, then rename)
    async fn save_to_file(path: &Path, json: &str) -> Result<(), PricingError> {
        let temp_path = path.with_extension("json.tmp");
        tokio::fs::write(&temp_path, json).await?;

        // Windows-safe atomic replace: remove destination first if exists
        #[cfg(target_os = "windows")]
        if path.exists() {
            let _ = tokio::fs::remove_file(path).await;
        }

        tokio::fs::rename(&temp_path, path).await?;
        Ok(())
    }

    /// Calculate costs for a span's token usage
    ///
    /// Thread-safe: acquires read lock on pricing data.
    /// Fail-safe: returns zero costs if model not found (debug log only).
    pub fn calculate_cost(&self, input: &SpanCostInput) -> SpanCostOutput {
        let model = match &input.model {
            Some(m) if !m.is_empty() => m.as_str(),
            _ => return SpanCostOutput::default(),
        };

        let data = self.data.read();
        let (pricing, match_type) = match data.lookup(input.system.as_deref(), model) {
            Some(result) => result,
            None => {
                tracing::trace!(
                    model = model,
                    system = input.system.as_deref().unwrap_or("none"),
                    "No pricing found for model"
                );
                return SpanCostOutput {
                    match_type: Some(MatchType::NotFound),
                    ..Default::default()
                };
            }
        };

        // For embedding models, only input tokens are charged
        let is_embedding = pricing.mode.eq_ignore_ascii_case("embedding");

        // Clamp token counts to prevent negative costs from data corruption
        let input_tokens = input.input_tokens.max(0) as f64;
        let output_tokens = input.output_tokens.max(0) as f64;
        let cache_read_tokens = input.cache_read_tokens.max(0) as f64;
        let cache_write_tokens = input.cache_write_tokens.max(0) as f64;
        let reasoning_tokens = input.reasoning_tokens.max(0) as f64;

        // Whether the provider's cache and reasoning counters are *subsets* of the input and output totals.
        //
        // This decides the bill, and the two conventions are irreconcilable:
        //
        //   * OpenAI (and every OpenAI-compatible endpoint) reports `prompt_tokens_details.cached_tokens`
        //     *within* `prompt_tokens`, and `completion_tokens_details.reasoning_tokens` *within*
        //     `completion_tokens`. Charging the totals and then the subsets again bills the cached portion
        //     twice - once at the full input rate, once at the cache rate. For a GPT-5 call with 100 input
        //     tokens of which 80 were cached that is ~4x the true cost, and the number is what a user makes
        //     spending decisions on.
        //   * Anthropic reports `cache_read_input_tokens` and `cache_creation_input_tokens` *beside*
        //     `input_tokens`, which excludes them. There the subsets must be added, and subtracting would
        //     under-report.
        //
        // So the counters are stored exactly as the provider reported them - the UI shows what the provider
        // said - and the *charge* is normalised here, where the provider is known. Unknown providers take the
        // inclusive reading, because OpenAI-compatible endpoints are the common case by a wide margin and
        // over-charging is the worse error to hand someone.
        // The conventions, keyed on the provider of the entry that **priced this call** rather than on a
        // second parse of `gen_ai.system`. The lookup resolves a provider from the model name as well as
        // the attribute, so the two answers differ exactly when the attribute is missing or spelled in a
        // way the mapper does not know - and then the call was charged at one provider's rates and counted
        // under another's convention. The resolved provider is reported back so the token total can be
        // derived from the same answer.
        let resolved_provider = pricing.litellm_provider.as_str();
        let cache_is_included = !cache_counters_are_separate_for_provider(resolved_provider);
        let reasoning_is_included = !reasoning_is_separate_for_provider(resolved_provider);

        // The portion charged at the plain input rate: everything not already billed as cache.
        let billable_input = if cache_is_included {
            (input_tokens - cache_read_tokens - cache_write_tokens).max(0.0)
        } else {
            input_tokens
        };
        let billable_output = if reasoning_is_included {
            (output_tokens - reasoning_tokens).max(0.0)
        } else {
            output_tokens
        };

        // Calculate costs
        let input_cost = billable_input * pricing.input_cost_per_token;

        // Output cost: zero for embeddings (they only have input)
        let output_cost = if is_embedding {
            0.0
        } else {
            billable_output * pricing.output_cost_per_token
        };

        let cache_read_cost = cache_read_tokens * pricing.cache_read_input_token_cost;
        let cache_write_cost = cache_write_tokens * pricing.cache_creation_input_token_cost;

        // Reasoning tokens: use dedicated rate if available, else output rate
        let reasoning_cost = if is_embedding {
            0.0
        } else {
            let reasoning_rate = if pricing.output_cost_per_reasoning_token > 0.0 {
                pricing.output_cost_per_reasoning_token
            } else {
                pricing.output_cost_per_token
            };
            reasoning_tokens * reasoning_rate
        };

        let total_cost =
            input_cost + output_cost + cache_read_cost + cache_write_cost + reasoning_cost;

        tracing::trace!(
            model = model,
            match_type = ?match_type,
            mode = pricing.mode,
            total_cost = total_cost,
            "Calculated cost"
        );

        SpanCostOutput {
            input_cost,
            output_cost,
            cache_read_cost,
            cache_write_cost,
            reasoning_cost,
            total_cost,
            match_type: Some(match_type),
            resolved_provider: (!resolved_provider.is_empty())
                .then(|| resolved_provider.to_string()),
        }
    }

    /// Get model pricing information (per-token rates)
    ///
    /// Returns the pricing rates and match type for a given model.
    /// Thread-safe: acquires read lock on pricing data.
    pub fn get_model_pricing(
        &self,
        provider: Option<&str>,
        model: &str,
    ) -> Option<(ModelPricing, MatchType)> {
        if model.is_empty() {
            return None;
        }

        let data = self.data.read();
        data.lookup(provider, model)
            .map(|(pricing, match_type)| (pricing.clone(), match_type))
    }

    /// Sync pricing data from GitHub
    async fn sync(&self) {
        let Some(source) = &self.catalogue_source else {
            return;
        };

        match source.fetch_catalogue().await {
            Ok(text) => self.apply_sync_data(&text).await,
            Err(error) => tracing::warn!(%error, "Pricing catalogue sync failed"),
        }
    }

    /// Apply synced data: parse, save to disk atomically, update memory
    pub(super) async fn apply_sync_data(&self, json: &str) {
        // Parse first to validate
        let new_data = match PricingData::from_json_str(json) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse synced pricing data");
                return;
            }
        };

        // Two guards, neither of them a ratio against whatever happens to be loaded.
        //
        // The old check refused a sync holding fewer than half the current catalogue's models. A model count
        // measures accumulated history, not correctness: a local catalogue bloated with retired models raised
        // the bar until the *current* upstream catalogue was refused, and since the check ran on every sync
        // the prices were pinned permanently - the worse the local file, the harder it was to fix.
        //
        // What the check was actually for is a truncated or wrong download, and that is asked directly:

        // 1. A structural floor. Fixed, so it cannot be dragged upward by a bad local file, and far below
        //    any genuine catalogue.
        if new_data.model_count < MIN_PLAUSIBLE_MODEL_COUNT {
            tracing::warn!(
                new = new_data.model_count,
                minimum = MIN_PLAUSIBLE_MODEL_COUNT,
                "Rejecting synced pricing: too few priced models to be a real catalogue"
            );
            return;
        }

        // There is deliberately no second check against "the models this instance is using".
        //
        // That was tried, and it made acceptance a function of the *replica* rather than of the catalogue:
        // replica A had priced model M and refused an upstream catalogue that dropped it, while replica B
        // had not and accepted the same catalogue. Cost is persisted at ingestion, so which price a span was
        // stored at then depended on which replica the balancer picked - the exact routing-dependence the
        // provenance rule above exists to remove. The observation set was also caller-fillable: a client
        // sending 256 junk model names displaced every real one and the check protected nothing.
        //
        // What is left is a decision about the catalogue alone, so every replica of a build reaches the same
        // one. The residual risk is a partially truncated catalogue that still holds more than the floor and
        // happens to drop a model in use; that leaves the model *unpriced* (cost 0, visible) rather than
        // mispriced, and the next sync corrects it.

        // Save to disk atomically
        if let Err(e) = Self::save_to_file(&self.local_path, json).await {
            tracing::warn!(error = %e, "Failed to save pricing data to disk");
        } else {
            Self::write_provenance(
                &self.local_path,
                PricingProvenance {
                    source: PROVENANCE_SYNC.to_string(),
                    // The build this sync was fetched under. A later build with a different snapshot
                    // must not keep pricing from a catalogue that predates its own corrections.
                    embedded_digest: Some(embedded_digest().to_string()),
                    written_at: self.clock.now().to_rfc3339(),
                },
            )
            .await;
        }

        // Update in-memory data
        {
            let mut data = self.data.write();
            *data = new_data;
        }
    }

    /// Start the background catalogue sync task.
    ///
    /// `sync_hours = 0` or an absent catalogue source disables the task. Enabled intervals are clamped to
    /// [`MIN_SYNC_HOURS`], and the first sync starts immediately.
    pub fn start_sync_task(
        self: &Arc<Self>,
        sync_hours: u64,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        if sync_hours == 0 || self.catalogue_source.is_none() {
            return None;
        }

        let sync_hours = sync_hours.max(MIN_SYNC_HOURS);
        let interval = Duration::from_secs(sync_hours.saturating_mul(3600));
        let service = Arc::clone(self);

        Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            // A fresh catalogue fetch supersedes missed intervals; catch-up bursts only duplicate traffic.
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = timer.tick() => {
                        service.sync().await;
                    }
                }
            }
        }))
    }
}
