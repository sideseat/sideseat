/// A second ClickHouse store beside the scale test's, holding the same rows with a codec variant, so the reads can be
/// timed against both in alternation: one load, the same rows, and the time of day shared between them.
///
/// `SIDESEAT_READ_PARITY_CODECS` names a file with one `table.column CODEC [setting=value ...]` a line - say
/// `otel_raw.record ZSTD(3) max_compress_block_size=4194304`, or `otel_spans.metadata LZ4` to time a column without
/// its codec. A's tables are merged to one part per partition, which resolves their replaced revisions, then copied
/// into `{database}_b` with each listed column, which must exist and change, given that codec and those column-level
/// settings, and B is merged too, so the stores differ in those columns alone: the test fails unless every partition
/// holds parts of the same rows in both. `SIDESEAT_READ_PARITY_ROUNDS` (default 5) is how many rounds each read is
/// timed for, A first in even rounds and B first in odd ones, so neither store always reads the cache the other warmed.
async fn codec_variant(
    url: &str,
    database: &str,
) -> Option<(sideseat_adapter_clickhouse::ClickhouseRepository, usize)> {
    let path = std::env::var("SIDESEAT_READ_PARITY_CODECS").ok()?;
    let mut listed = std::collections::BTreeMap::new();
    for line in std::fs::read_to_string(&path)
        .expect("the codec variant's column list")
        .lines()
    {
        let mut words = line.split_whitespace();
        let Some(column) = words.next() else {
            continue;
        };
        let codec = words.next().unwrap_or_else(|| panic!("{column}: no codec"));
        let settings: Vec<String> = words.map(|setting| setting.replace('=', " = ")).collect();
        let previous = listed.insert(column.to_string(), (codec.to_string(), settings));
        assert!(previous.is_none(), "{column} is listed twice");
    }
    let rounds = std::env::var("SIDESEAT_READ_PARITY_ROUNDS")
        .ok()
        .map(|rounds| rounds.parse().expect("a round count"))
        .unwrap_or(5);
    let variant = format!("{database}_b");
    let repo = clickhouse_backend(url, &variant).await;
    let raw = raw_client(url, &variant).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );
    let columns: Vec<(String, String, String, String)> = raw
        .query(
            "SELECT table, name, type, compression_codec FROM system.columns \
             WHERE database = currentDatabase() ORDER BY table, position",
        )
        .fetch_all()
        .await
        .expect("the variant's columns");
    for (table, name, kind, current) in &columns {
        let Some((codec, settings)) = listed.remove(&format!("{table}.{name}")) else {
            continue;
        };
        let settings = if settings.is_empty() {
            String::new()
        } else {
            format!(" SETTINGS ({})", settings.join(", "))
        };
        let definition = format!("{name} {kind} CODEC({codec}){settings}");
        let created = || async {
            raw.query("SELECT create_table_query FROM system.tables WHERE database = currentDatabase() AND name = ?")
                .bind(table.as_str())
                .fetch_one::<String>()
                .await
                .expect("the table's definition")
        };
        let before = created().await;
        raw.query(&format!("ALTER TABLE {table} MODIFY COLUMN {definition}"))
            .execute()
            .await
            .unwrap_or_else(|error| panic!("{table}.{name}: {error}"));
        assert_ne!(
            before,
            created().await,
            "{table}.{definition} changes nothing"
        );
        println!("codec variant: {table}.{definition}, was {current:?}");
    }
    assert!(listed.is_empty(), "not in the store: {:?}", listed.keys());
    let tables: std::collections::BTreeSet<&str> =
        columns.iter().map(|(table, ..)| table.as_str()).collect();
    for table in &tables {
        let merge = |store: &str| format!("OPTIMIZE TABLE {store}.{table} FINAL");
        for statement in [
            merge(database),
            format!("INSERT INTO {variant}.{table} SELECT * FROM {database}.{table}"),
            merge(&variant),
        ] {
            raw.query(&statement)
                .execute()
                .await
                .unwrap_or_else(|error| panic!("{statement}: {error}"));
        }
    }
    print_part_layouts(&raw, database, &variant, &tables).await;
    println!("codec variant: {variant}, {rounds} A/B rounds a read");
    Some((repo, rounds))
}

/// Every table's active parts in both stores, by partition and rows, which must be the same.
async fn print_part_layouts(
    raw: &clickhouse::Client,
    database: &str,
    variant: &str,
    tables: &std::collections::BTreeSet<&str>,
) {
    let parts: Vec<(String, String, String, u64)> = raw
        .query(
            "SELECT database, table, partition_id, rows FROM system.parts \
             WHERE active AND database IN (?, ?) ORDER BY table, partition_id, rows",
        )
        .bind(database)
        .bind(variant)
        .fetch_all()
        .await
        .expect("the stores' parts");
    println!("\n| table | parts | rows | per part |\n|---|---|---|---|");
    for table in tables {
        let of = |store: &str| -> Vec<(&str, u64)> {
            parts
                .iter()
                .filter(|(db, name, ..)| db == store && name == *table)
                .map(|(.., partition, rows)| (partition.as_str(), *rows))
                .collect()
        };
        let (a, b) = (of(database), of(variant));
        assert_eq!(a, b, "{table}: the variant's parts differ from A's");
        let rows: u64 = a.iter().map(|(_, rows)| rows).sum();
        let per_part: Vec<u64> = a.iter().map(|(_, rows)| *rows).collect();
        println!("| {table} | {} | {rows} | {per_part:?} |", a.len());
    }
}

/// The A/B timings of every read, each the median of its rounds' fastest-of-three, and the median of their ratios.
fn print_codec_variant(ab: &[(String, Vec<std::time::Duration>, Vec<std::time::Duration>)]) {
    if ab.is_empty() {
        return;
    }
    let median = |times: &[std::time::Duration]| -> f64 {
        let mut ms: Vec<f64> = times.iter().map(|t| t.as_secs_f64() * 1000.0).collect();
        ms.sort_by(f64::total_cmp);
        ms[ms.len() / 2]
    };
    println!("\n| read | ClickHouse A | ClickHouse B | B/A |\n|---|---|---|---|");
    let mut ratios = Vec::new();
    let mut spreads = Vec::new();
    for (name, a, b) in ab {
        let fastest = a.iter().min().expect("a round").as_secs_f64();
        let slowest = a.iter().max().expect("a round").as_secs_f64();
        spreads.push(slowest / fastest);
        let (a, b) = (median(a), median(b));
        ratios.push(b / a);
        println!("| {name} | {a:.1} ms | {b:.1} ms | {:.3} |", b / a);
    }
    ratios.sort_by(f64::total_cmp);
    spreads.sort_by(f64::total_cmp);
    println!(
        "A's own spread, slowest round over fastest: median {:.3} over the reads",
        spreads[spreads.len() / 2]
    );
    println!(
        "\nB/A over {} reads: median {:.3}, lowest {:.3}, highest {:.3}",
        ratios.len(),
        ratios[ratios.len() / 2],
        ratios[0],
        ratios[ratios.len() - 1]
    );
}

/// `rounds` timings of a read on A and on B, A first in even rounds and B first in odd ones, each the fastest of three
/// runs, with every run's answer, so none is checked only when it happens to be the fastest.
async fn alternate_rounds<T, FA, FB, FutA, FutB>(
    rounds: usize,
    a: FA,
    b: FB,
) -> (Vec<std::time::Duration>, Vec<std::time::Duration>, Vec<T>)
where
    FA: Fn() -> FutA,
    FB: Fn() -> FutB,
    FutA: std::future::Future<Output = T>,
    FutB: std::future::Future<Output = T>,
{
    async fn three<T, Fut: std::future::Future<Output = T>>(
        read: impl Fn() -> Fut,
        answers: &mut Vec<T>,
    ) -> std::time::Duration {
        let mut fastest = std::time::Duration::MAX;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let answer = read().await;
            fastest = fastest.min(started.elapsed());
            answers.push(answer);
        }
        fastest
    }
    let (mut a_times, mut b_times, mut answers) = (Vec::new(), Vec::new(), Vec::new());
    for round in 0..rounds {
        if round % 2 == 1 {
            b_times.push(three(&b, &mut answers).await);
        }
        a_times.push(three(&a, &mut answers).await);
        if round % 2 == 0 {
            b_times.push(three(&b, &mut answers).await);
        }
    }
    (a_times, b_times, answers)
}
