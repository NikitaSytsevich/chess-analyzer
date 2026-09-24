//! Разбор строк, которые присылает UCI-движок. Только чистые функции: их
//! легко проверить на записанном выводе настоящего Stockfish.

/// Строка `info` с линией анализа: оценка и главный вариант.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoLine {
    pub depth: u32,
    pub seldepth: u32,
    pub multipv: u8,
    pub score: RawScore,
    /// Выигрыш, ничья, проигрыш стороны, чей ход, в промилле.
    pub wdl: Option<(u16, u16, u16)>,
    pub nodes: u64,
    pub nps: u64,
    pub hashfull: u32,
    pub time_ms: u64,
    /// Ходы варианта в формате UCI: `g1f3`, `e7e8q`.
    pub pv: Vec<String>,
}

/// Оценка как её прислал движок — со стороны того, чей ход.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawScore {
    Cp(i32),
    Mate(i32),
}

/// Что за строка пришла от движка.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineLine {
    UciOk,
    ReadyOk,
    IdName(String),
    Info(InfoLine),
    BestMove(Option<String>),
    /// Всё остальное: `info string …`, строки без варианта, опции.
    Other,
}

pub fn parse_line(line: &str) -> EngineLine {
    let mut words = line.split_ascii_whitespace();
    match words.next() {
        Some("uciok") => EngineLine::UciOk,
        Some("readyok") => EngineLine::ReadyOk,
        Some("id") if words.next() == Some("name") => EngineLine::IdName(words.collect::<Vec<_>>().join(" ")),
        Some("bestmove") => {
            EngineLine::BestMove(words.next().filter(|mv| *mv != "(none)").map(str::to_owned))
        }
        Some("info") => parse_info(words).map_or(EngineLine::Other, EngineLine::Info),
        _ => EngineLine::Other,
    }
}

fn parse_info<'a>(mut words: impl Iterator<Item = &'a str>) -> Option<InfoLine> {
    let mut depth = None;
    let mut seldepth = 0;
    let mut multipv = 1;
    let mut score = None;
    let mut bound = false;
    let mut wdl = None;
    let mut nodes = 0;
    let mut nps = 0;
    let mut hashfull = 0;
    let mut time_ms = 0;
    let mut pv = Vec::new();

    while let Some(word) = words.next() {
        match word {
            "depth" => depth = words.next()?.parse().ok(),
            "seldepth" => seldepth = words.next()?.parse().ok()?,
            "multipv" => multipv = words.next()?.parse().ok()?,
            "score" => {
                score = match words.next()? {
                    "cp" => Some(RawScore::Cp(words.next()?.parse().ok()?)),
                    "mate" => Some(RawScore::Mate(words.next()?.parse().ok()?)),
                    _ => return None,
                };
            }
            // Оценка вне окна поиска — лишь граница, а не значение. Такие
            // строки заставили бы шкалу дёргаться на каждой итерации.
            "lowerbound" | "upperbound" => bound = true,
            "wdl" => {
                let w = words.next()?.parse().ok()?;
                let d = words.next()?.parse().ok()?;
                let l = words.next()?.parse().ok()?;
                wdl = Some((w, d, l));
            }
            "nodes" => nodes = words.next()?.parse().ok()?,
            "nps" => nps = words.next()?.parse().ok()?,
            "hashfull" => hashfull = words.next()?.parse().ok()?,
            "time" => time_ms = words.next()?.parse().ok()?,
            "tbhits" | "cpuload" | "currmovenumber" => {
                words.next();
            }
            // Текстовое сообщение движка — не линия анализа.
            "string" | "currmove" | "refutation" | "currline" => return None,
            "pv" => {
                pv = words.by_ref().map(str::to_owned).collect();
                break;
            }
            _ => {}
        }
    }
    if bound || pv.is_empty() {
        return None;
    }
    Some(InfoLine { depth: depth?, seldepth, multipv, score: score?, wdl, nodes, nps, hashfull, time_ms, pv })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Строки из настоящего вывода Stockfish 19 после 1.e4 e5.
    const DEPTH_12: &str = "info depth 12 seldepth 21 multipv 1 score cp 27 wdl 52 942 6 nodes 154571 nps 2493080 hashfull 42 tbhits 0 time 62 pv g1f3 b8c6 d2d4 e5d4";

    #[test]
    fn a_full_info_line_is_read_completely() {
        let EngineLine::Info(info) = parse_line(DEPTH_12) else { panic!("not an info line") };
        assert_eq!(info.depth, 12);
        assert_eq!(info.seldepth, 21);
        assert_eq!(info.multipv, 1);
        assert_eq!(info.score, RawScore::Cp(27));
        assert_eq!(info.wdl, Some((52, 942, 6)));
        assert_eq!(info.nps, 2_493_080);
        assert_eq!(info.hashfull, 42);
        assert_eq!(info.time_ms, 62);
        assert_eq!(info.pv, ["g1f3", "b8c6", "d2d4", "e5d4"]);
    }

    #[test]
    fn service_lines_are_not_analysis() {
        assert_eq!(parse_line("info string NNUE evaluation using nn-1a298aa575a0.nnue"), EngineLine::Other);
        assert_eq!(parse_line("info depth 5 currmove e2e4 currmovenumber 1"), EngineLine::Other);
        // Граница окна поиска — не оценка.
        assert_eq!(
            parse_line("info depth 20 multipv 1 score cp 35 lowerbound nodes 10 nps 10 time 1 pv e2e4"),
            EngineLine::Other
        );
    }

    #[test]
    fn mates_and_best_moves() {
        let EngineLine::Info(info) =
            parse_line("info depth 30 multipv 1 score mate -3 nodes 1 nps 1 time 1 pv h7h6 d1d8")
        else {
            panic!("not an info line")
        };
        assert_eq!(info.score, RawScore::Mate(-3));
        assert_eq!(parse_line("bestmove g1f3 ponder b8c6"), EngineLine::BestMove(Some("g1f3".into())));
        assert_eq!(parse_line("bestmove (none)"), EngineLine::BestMove(None));
        assert_eq!(parse_line("id name Stockfish 19"), EngineLine::IdName("Stockfish 19".into()));
        assert_eq!(parse_line("uciok"), EngineLine::UciOk);
    }
}
