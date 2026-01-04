use crate::models::{AllTimeStats, Position, Trade};
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use tracing::info;

pub struct Database {
    conn: Connection,
}

// SAFETY: Database is always wrapped in Arc<RwLock<Database>>, ensuring
// that only one thread can access the connection at a time. SQLite's
// connection is not Send, but we ensure thread-safe access via RwLock.
unsafe impl Send for Database {}
unsafe impl Sync for Database {}

impl Database {
    pub fn new(db_path: &str) -> Result<Self> {
        // Create directory if it doesn't exist
        if let Some(parent) = Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path).context("Failed to open database")?;

        let mut db = Self { conn };
        db.initialize_tables()?;

        Ok(db)
    }

    fn initialize_tables(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                strategy TEXT NOT NULL,
                token_address TEXT NOT NULL,
                token_symbol TEXT,
                entry_price REAL NOT NULL,
                exit_price REAL,
                amount_sol REAL NOT NULL,
                tokens_bought REAL NOT NULL,
                tokens_sold REAL,
                entry_time INTEGER NOT NULL,
                exit_time INTEGER,
                pnl_sol REAL,
                pnl_percent REAL,
                status TEXT DEFAULT 'open',
                tx_signature_buy TEXT,
                tx_signature_sell TEXT,
                fees_sol REAL DEFAULT 0,
                notes TEXT
            );

            CREATE TABLE IF NOT EXISTS portfolio_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                total_value_sol REAL NOT NULL,
                available_sol REAL NOT NULL,
                open_positions INTEGER NOT NULL,
                daily_pnl REAL,
                total_pnl REAL
            );

            CREATE TABLE IF NOT EXISTS risk_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                severity TEXT NOT NULL,
                description TEXT,
                action_taken TEXT
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);
            CREATE INDEX IF NOT EXISTS idx_trades_token ON trades(token_address);
            CREATE INDEX IF NOT EXISTS idx_trades_entry_time ON trades(entry_time);
            ",
        )?;

        info!("Database initialized");
        Ok(())
    }

    pub fn record_trade_entry(
        &mut self,
        strategy: &str,
        token_address: &str,
        token_symbol: &str,
        entry_price: f64,
        amount_sol: f64,
        tokens_bought: f64,
        tx_signature: &str,
        fees: f64,
    ) -> Result<i64> {
        let entry_time = chrono::Utc::now().timestamp_millis();

        self.conn.execute(
            "INSERT INTO trades (
                strategy, token_address, token_symbol, entry_price,
                amount_sol, tokens_bought, entry_time, tx_signature_buy, fees_sol
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                strategy,
                token_address,
                token_symbol,
                entry_price,
                amount_sol,
                tokens_bought,
                entry_time,
                tx_signature,
                fees
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn record_trade_exit(
        &mut self,
        trade_id: i64,
        exit_price: f64,
        tokens_sold: f64,
        pnl_sol: f64,
        pnl_percent: f64,
        tx_signature: &str,
        fees: f64,
    ) -> Result<()> {
        let exit_time = chrono::Utc::now().timestamp_millis();

        self.conn.execute(
            "UPDATE trades SET
                exit_price = ?1,
                tokens_sold = ?2,
                exit_time = ?3,
                pnl_sol = ?4,
                pnl_percent = ?5,
                status = 'closed',
                tx_signature_sell = ?6,
                fees_sol = fees_sol + ?7
            WHERE id = ?8",
            params![
                exit_price,
                tokens_sold,
                exit_time,
                pnl_sol,
                pnl_percent,
                tx_signature,
                fees,
                trade_id
            ],
        )?;

        Ok(())
    }

    pub fn get_open_positions(&self) -> Result<Vec<Trade>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM trades WHERE status = 'open' ORDER BY entry_time DESC")?;

        let trades = stmt
            .query_map([], |row| {
                Ok(Trade {
                    id: row.get(0)?,
                    strategy: row.get(1)?,
                    token_address: row.get(2)?,
                    token_symbol: row.get(3)?,
                    entry_price: row.get(4)?,
                    exit_price: row.get(5)?,
                    amount_sol: row.get(6)?,
                    tokens_bought: row.get(7)?,
                    tokens_sold: row.get(8)?,
                    entry_time: row.get(9)?,
                    exit_time: row.get(10)?,
                    pnl_sol: row.get(11)?,
                    pnl_percent: row.get(12)?,
                    status: row.get(13)?,
                    tx_signature_buy: row.get(14)?,
                    tx_signature_sell: row.get(15)?,
                    fees_sol: row.get(16)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(trades)
    }

    pub fn get_position_by_token(&self, token_address: &str) -> Result<Option<Trade>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM trades WHERE token_address = ?1 AND status = 'open' LIMIT 1",
        )?;

        let mut rows = stmt.query(params![token_address])?;

        if let Some(row) = rows.next()? {
            Ok(Some(Trade {
                id: row.get(0)?,
                strategy: row.get(1)?,
                token_address: row.get(2)?,
                token_symbol: row.get(3)?,
                entry_price: row.get(4)?,
                exit_price: row.get(5)?,
                amount_sol: row.get(6)?,
                tokens_bought: row.get(7)?,
                tokens_sold: row.get(8)?,
                entry_time: row.get(9)?,
                exit_time: row.get(10)?,
                pnl_sol: row.get(11)?,
                pnl_percent: row.get(12)?,
                status: row.get(13)?,
                tx_signature_buy: row.get(14)?,
                tx_signature_sell: row.get(15)?,
                fees_sol: row.get(16)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn log_risk_event(
        &mut self,
        event_type: &str,
        severity: &str,
        description: &str,
        action_taken: &str,
    ) -> Result<()> {
        let timestamp = chrono::Utc::now().timestamp_millis();

        self.conn.execute(
            "INSERT INTO risk_events (timestamp, event_type, severity, description, action_taken)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![timestamp, event_type, severity, description, action_taken],
        )?;

        Ok(())
    }

    pub fn get_all_trades(&self) -> Result<Vec<Trade>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM trades WHERE status = 'closed'")?;

        let trades: Vec<Trade> = stmt
            .query_map([], |row| {
                Ok(Trade {
                    id: row.get(0)?,
                    strategy: row.get(1)?,
                    token_address: row.get(2)?,
                    token_symbol: row.get(3)?,
                    entry_price: row.get(4)?,
                    exit_price: row.get(5)?,
                    amount_sol: row.get(6)?,
                    tokens_bought: row.get(7)?,
                    tokens_sold: row.get(8)?,
                    entry_time: row.get(9)?,
                    exit_time: row.get(10)?,
                    pnl_sol: row.get(11)?,
                    pnl_percent: row.get(12)?,
                    status: row.get(13)?,
                    tx_signature_buy: row.get(14)?,
                    tx_signature_sell: row.get(15)?,
                    fees_sol: row.get(16)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(trades)
    }

    pub fn get_all_time_stats(&self) -> Result<AllTimeStats> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM trades WHERE status = 'closed'")?;

        let trades: Vec<Trade> = stmt
            .query_map([], |row| {
                Ok(Trade {
                    id: row.get(0)?,
                    strategy: row.get(1)?,
                    token_address: row.get(2)?,
                    token_symbol: row.get(3)?,
                    entry_price: row.get(4)?,
                    exit_price: row.get(5)?,
                    amount_sol: row.get(6)?,
                    tokens_bought: row.get(7)?,
                    tokens_sold: row.get(8)?,
                    entry_time: row.get(9)?,
                    exit_time: row.get(10)?,
                    pnl_sol: row.get(11)?,
                    pnl_percent: row.get(12)?,
                    status: row.get(13)?,
                    tx_signature_buy: row.get(14)?,
                    tx_signature_sell: row.get(15)?,
                    fees_sol: row.get(16)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        if trades.is_empty() {
            return Ok(AllTimeStats {
                total_trades: 0,
                win_count: 0,
                loss_count: 0,
                win_rate: 0.0,
                total_pnl: 0.0,
                avg_pnl: 0.0,
                best_trade: 0.0,
                worst_trade: 0.0,
                avg_hold_time_minutes: 0.0,
            });
        }

        let win_count = trades.iter().filter(|t| t.pnl_sol.unwrap_or(0.0) > 0.0).count();
        let total_pnl: f64 = trades.iter().map(|t| t.pnl_sol.unwrap_or(0.0)).sum();
        let best_trade = trades
            .iter()
            .map(|t| t.pnl_sol.unwrap_or(0.0))
            .fold(f64::NEG_INFINITY, f64::max);
        let worst_trade = trades
            .iter()
            .map(|t| t.pnl_sol.unwrap_or(0.0))
            .fold(f64::INFINITY, f64::min);

        // Calculate average hold time
        let total_hold_time: i64 = trades.iter()
            .filter_map(|t| {
                let exit = t.exit_time?;
                let entry = t.entry_time;
                Some(exit - entry)
            })
            .sum();
        let avg_hold_time_minutes = if !trades.is_empty() {
            (total_hold_time as f64 / trades.len() as f64) / (1000.0 * 60.0)
        } else {
            0.0
        };

        Ok(AllTimeStats {
            total_trades: trades.len(),
            win_count,
            loss_count: trades.len() - win_count,
            win_rate: (win_count as f64 / trades.len() as f64) * 100.0,
            total_pnl,
            avg_pnl: total_pnl / trades.len() as f64,
            best_trade,
            worst_trade,
            avg_hold_time_minutes,
        })
    }

    pub fn update_capital(&mut self, new_capital: f64) -> Result<()> {
        // Store capital in a settings table or config
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )?;

        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('current_capital', ?1)",
            params![new_capital.to_string()],
        )?;

        Ok(())
    }

    pub fn get_current_capital(&self, default: f64) -> Result<f64> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM settings WHERE key = 'current_capital'")?;

        let mut rows = stmt.query([])?;

        if let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            Ok(value.parse().unwrap_or(default))
        } else {
            Ok(default)
        }
    }
    // Database query methods removed - use existing get_all_trades() and filter in code if needed

}
