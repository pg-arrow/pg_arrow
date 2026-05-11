/// PostgreSQL transaction snapshot for MVCC visibility checks.
///
/// Obtained from `txid_current_snapshot()` inside a REPEATABLE READ transaction.
/// Format: `xmin:xmax:xip_list` e.g. `"100:105:101,103"`.
#[derive(Debug, Clone, Default)]
pub struct PgSnapshot {
    /// All xids < xmin are committed and visible.
    pub xmin: u32,
    /// All xids >= xmax are not yet assigned and invisible.
    pub xmax: u32,
    /// Xids in [xmin, xmax) that were in-progress at snapshot time (not visible).
    pub xip: Vec<u32>,
}

/// PostgreSQL frozen transaction ID — always visible.
const FROZEN_XID: u32 = 2;

impl PgSnapshot {
    /// Parse PostgreSQL snapshot string: `"xmin:xmax:xip_list"`.
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.splitn(3, ':').collect();
        if parts.len() < 2 {
            return None;
        }
        let xmin = parts[0].trim().parse::<u32>().ok()?;
        let xmax = parts[1].trim().parse::<u32>().ok()?;
        let xip = if parts.len() == 3 && !parts[2].trim().is_empty() {
            parts[2]
                .split(',')
                .filter_map(|x| x.trim().parse::<u32>().ok())
                .collect()
        } else {
            vec![]
        };
        Some(Self { xmin, xmax, xip })
    }

    /// Returns true if `t_xmin` is visible under this snapshot.
    ///
    /// Rules (simplified, assumes tuple is committed — caller already checked xmax):
    ///   - frozen xid (≤ 2)         → always visible
    ///   - t_xmin < xmin            → committed before snapshot → visible
    ///   - t_xmin >= xmax           → started after snapshot → invisible
    ///   - t_xmin in xip            → in-progress at snapshot → invisible
    ///   - otherwise                → visible
    #[inline]
    pub fn xmin_visible(&self, t_xmin: u32) -> bool {
        if t_xmin <= FROZEN_XID {
            return true;
        }
        if t_xmin < self.xmin {
            return true;
        }
        if t_xmin >= self.xmax {
            return false;
        }
        !self.xip.contains(&t_xmin)
    }
}
