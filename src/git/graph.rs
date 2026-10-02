//! Tata letak graf commit ala Git Graph: setiap commit menempati satu baris,
//! satu lane (kolom) untuk node-nya, dan garis ke parent digambar per setengah
//! baris.
//!
//! Algoritmanya murni (tanpa git) supaya bisa dites: masukan adalah daftar
//! `(hash, parents)` yang sudah berurutan anak-sebelum-parent seperti keluaran
//! `git log --date-order` / `--topo-order`.
//!
//! Setiap lane menunggu satu hash (commit yang akan datang). Saat commit tiba:
//! - lane pertama yang menunggunya menjadi lane node; lane lain yang juga
//!   menunggu hash itu bergabung ke node (garis setengah atas melengkung),
//! - parent pertama meneruskan lane node (warna sama), kecuali parent itu
//!   sudah ditunggu lane lain, maka garis diarahkan ke lane tersebut,
//! - parent berikutnya (merge) memakai lane yang sudah menunggunya atau lane
//!   baru dengan warna baru.

/// Satu segmen garis di dalam satu baris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Lane di ujung awal segmen.
    pub from: usize,
    /// Lane di ujung akhir segmen.
    pub to: usize,
    /// Indeks warna (diputar oleh UI pada paletnya).
    pub color: usize,
    /// `true` = setengah atas (tepi atas → tengah); `false` = setengah bawah
    /// (tengah → tepi bawah).
    pub top: bool,
}

/// Hasil tata letak satu baris.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    pub segments: Vec<Segment>,
    /// Jumlah lane yang terpakai di baris ini (untuk lebar kolom graf).
    pub width: usize,
}

#[derive(Debug, Clone)]
struct Lane {
    waiting: String,
    color: usize,
}

/// Susun graf. `commits` berisi `(hash, parents)`; parent yang tidak ada di
/// daftar (halaman berikutnya belum dimuat) tetap digambar sebagai garis yang
/// keluar dari bawah.
pub fn layout<'a>(commits: impl IntoIterator<Item = (&'a str, &'a [String])>) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::new();

    for (hash, parents) in commits {
        let mut row = GraphRow::default();

        // Lane yang menunggu commit ini.
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| l.waiting == hash))
            .map(|(i, _)| i)
            .collect();
        let (node, color) = match waiting.first() {
            Some(&i) => (i, lanes[i].as_ref().map_or(0, |l| l.color)),
            None => {
                let i = free_slot(&mut lanes);
                let c = next_color;
                next_color += 1;
                lanes[i] = Some(Lane {
                    waiting: hash.to_string(),
                    color: c,
                });
                (i, c)
            }
        };
        row.lane = node;
        row.color = color;

        // Setengah atas: semua lane aktif turun ke tengah; yang menunggu commit
        // ini menuju node.
        for (i, l) in lanes.iter().enumerate() {
            let Some(l) = l else { continue };
            if l.waiting == hash {
                if waiting.contains(&i) {
                    row.segments.push(Segment {
                        from: i,
                        to: node,
                        color: l.color,
                        top: true,
                    });
                }
            } else {
                row.segments.push(Segment {
                    from: i,
                    to: i,
                    color: l.color,
                    top: true,
                });
            }
        }
        // Lane gabungan selain lane node selesai di sini.
        for &i in waiting.iter().skip(1) {
            lanes[i] = None;
        }

        // Parent.
        let mut node_continues = false;
        let mut bottom_targets: Vec<(usize, usize)> = Vec::new();
        for (pi, parent) in parents.iter().enumerate() {
            // Parent pertama selalu meneruskan lane node, walau lane lain juga
            // menunggunya: lane-lane itu baru bertemu di baris parent (seperti
            // Git Graph), sehingga jalur utama tidak bergeser ke kanan.
            let existing = (pi > 0)
                .then(|| {
                    lanes.iter().enumerate().position(|(i, l)| {
                        i != node && l.as_ref().is_some_and(|l| l.waiting == *parent)
                    })
                })
                .flatten();
            match existing {
                Some(target) => {
                    let c = lanes[target].as_ref().map_or(color, |l| l.color);
                    bottom_targets.push((target, c));
                }
                None if pi == 0 => {
                    lanes[node] = Some(Lane {
                        waiting: parent.clone(),
                        color,
                    });
                    node_continues = true;
                    bottom_targets.push((node, color));
                }
                None => {
                    let i = free_slot_excluding(&mut lanes, node, node_continues);
                    let c = next_color;
                    next_color += 1;
                    lanes[i] = Some(Lane {
                        waiting: parent.clone(),
                        color: c,
                    });
                    bottom_targets.push((i, c));
                }
            }
        }
        if !node_continues {
            lanes[node] = None;
        }

        // Setengah bawah: lane yang lewat tetap lurus; node bercabang ke parent.
        for (i, l) in lanes.iter().enumerate() {
            let Some(l) = l else { continue };
            if bottom_targets.iter().any(|(t, _)| *t == i) {
                continue;
            }
            row.segments.push(Segment {
                from: i,
                to: i,
                color: l.color,
                top: false,
            });
        }
        for (target, c) in bottom_targets {
            row.segments.push(Segment {
                from: node,
                to: target,
                color: c,
                top: false,
            });
            // Lane target yang sudah ada sebelumnya juga harus terus turun.
            if target != node
                && let Some(l) = &lanes[target]
                && !row
                    .segments
                    .iter()
                    .any(|s| !s.top && s.from == target && s.to == target)
            {
                row.segments.push(Segment {
                    from: target,
                    to: target,
                    color: l.color,
                    top: false,
                });
            }
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        row.width = row
            .segments
            .iter()
            .map(|s| s.from.max(s.to) + 1)
            .max()
            .unwrap_or(0)
            .max(node + 1);
        rows.push(row);
    }
    rows
}

fn free_slot(lanes: &mut Vec<Option<Lane>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// Slot kosong selain `node` (bila node masih dipakai parent pertama).
fn free_slot_excluding(lanes: &mut Vec<Option<Lane>>, node: usize, node_taken: bool) -> usize {
    match lanes
        .iter()
        .enumerate()
        .position(|(i, l)| l.is_none() && !(node_taken && i == node) && i > node)
    {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(list: &[(&str, &[&str])]) -> Vec<GraphRow> {
        let owned: Vec<(String, Vec<String>)> = list
            .iter()
            .map(|(h, p)| (h.to_string(), p.iter().map(|s| s.to_string()).collect()))
            .collect();
        layout(owned.iter().map(|(h, p)| (h.as_str(), p.as_slice())))
    }

    #[test]
    fn linear_history_stays_in_lane_zero() {
        let rows = run(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        assert!(rows.iter().all(|r| r.lane == 0 && r.width == 1));
        assert_eq!(rows[0].color, rows[2].color);
        // Commit akar tidak punya garis ke bawah.
        assert!(rows[2].segments.iter().all(|s| s.top));
    }

    #[test]
    fn merge_opens_and_closes_a_lane() {
        // m merge dari b (lane 0) dan f (lane 1); f dan b turun ke a.
        let rows = run(&[("m", &["b", "f"]), ("f", &["a"]), ("b", &["a"]), ("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert!(
            rows[0]
                .segments
                .iter()
                .any(|s| !s.top && s.from == 0 && s.to == 1)
        );
        assert_eq!(rows[1].lane, 1);
        assert_ne!(rows[1].color, rows[0].color);
        assert_eq!(rows[2].lane, 0);
        // a ditunggu oleh dua lane: lane 1 bergabung ke node di lane 0.
        assert_eq!(rows[3].lane, 0);
        assert!(
            rows[3]
                .segments
                .iter()
                .any(|s| s.top && s.from == 1 && s.to == 0)
        );
        assert_eq!(rows[3].width, 2);
    }

    #[test]
    fn separate_tips_get_their_own_lanes() {
        let rows = run(&[("x", &["a"]), ("y", &["a"]), ("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1);
        // Lane 0 tetap lurus melewati baris y.
        assert!(
            rows[1]
                .segments
                .iter()
                .any(|s| s.top && s.from == 0 && s.to == 0)
        );
        assert_eq!(rows[2].lane, 0);
    }

    #[test]
    fn unloaded_parent_leaves_open_line() {
        let rows = run(&[("b", &["a"])]);
        assert!(
            rows[0]
                .segments
                .iter()
                .any(|s| !s.top && s.from == 0 && s.to == 0)
        );
    }

    #[test]
    fn octopus_merge_uses_three_lanes() {
        let rows = run(&[
            ("m", &["a", "b", "c"]),
            ("c", &["r"]),
            ("b", &["r"]),
            ("a", &["r"]),
            ("r", &[]),
        ]);
        assert_eq!(rows[0].width, 3);
        assert_eq!(rows[4].lane, 0);
    }
}
