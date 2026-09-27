//! Партия по потоку распознанных досок.
//!
//! Распознавание отвечает на вопрос «что на доске в этом кадре», трекер — «что
//! произошло в партии». Он ждёт, пока расстановка устоится (анимация хода,
//! мелькнувший курсор и стрелки комментатора не должны считаться ходами), и
//! ищет самое правдоподобное объяснение: ход, два хода, откат или новая
//! позиция. Шахматные правила — его главный фильтр: из позиции ведут только
//! легальные ходы, и ошибка распознавания в одной клетке не превращается в
//! «ход» ладьёй сквозь фигуры.

mod infer;
mod stable;

use std::time::{Duration, Instant};

use analyzer_chess::{Chess, Game, Move, SanPlus};
use analyzer_vision::{Observation, Orientation};

pub use crate::infer::{Explanation, board_to_position, explain};
use crate::stable::Stabilizer;

#[derive(Clone, Copy, Debug)]
pub struct TrackerConfig {
    /// Расстановка должна продержаться столько кадров…
    pub stable_frames: u32,
    /// …и столько времени, чтобы её приняли.
    pub stable_time: Duration,
    /// Клетки неувереннее этого не участвуют в сравнении кадров между собой.
    pub unsure: f32,
    /// Если ни одно объяснение не подходит столько времени — начинаем
    /// партию заново с того, что видно.
    pub resync_after: Duration,
    /// На сколько полуходов назад ищется откат.
    pub takeback_plies: usize,
}

/// Расстановку принимают, как только два кадра подряд показали одно и то же.
/// Захват присылает не больше десяти кадров в секунду, так что ход
/// принимается через 0,1 с после того, как фигура встала, — а не через
/// 0,3 с, как при трёх кадрах и 250 мс раньше. Подобрано прогоном партий
/// кадр за кадром, с анимацией ходов лайчесса и chess.com, премувами и
/// темпом пули (`cargo run --release -p analyzer-tracker --example settle`,
/// итоги — в README): два кадра — самое быстрое, что ни разу не приняло за
/// ход кадр посреди анимации. Один кадр принимает фигуру в полёте, а три и
/// больше в пуле пропускают ходы: соперник отвечает раньше, чем устоится
/// позиция, и за один раз набирается больше двух ходов — трекер теряет
/// партию. Время — меньше интервала между кадрами (100 мс), чтобы дрожание
/// меток времени не требовало третьего кадра.
impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            stable_frames: 2,
            stable_time: Duration::from_millis(80),
            unsure: 0.2,
            resync_after: Duration::from_millis(1500),
            takeback_plies: 10,
        }
    }
}

/// Что произошло в партии.
#[derive(Clone, Debug, PartialEq)]
pub enum GameEvent {
    /// Партия начата с этой позиции: стартовой или распознанной посреди игры.
    Started { position: Chess, reason: StartReason },
    /// Сыгран ход; `ply` — номер полухода после него.
    Moved { ply: usize, mv: Move, san: SanPlus },
    /// Трансляция откатила партию до `ply` полуходов.
    TookBack { to_ply: usize },
    /// Видимая расстановка не объясняется партией; если так останется,
    /// через `TrackerConfig::resync_after` будет пересинхронизация.
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartReason {
    /// Трекер только подключился.
    First,
    /// Показали другую позицию или новую партию.
    Resync,
    /// Позицию задал комментатор.
    Manual,
}

pub struct Tracker {
    config: TrackerConfig,
    game: Option<Game>,
    stabilizer: Stabilizer,
    /// С какого момента устойчивая расстановка ничем не объясняется.
    lost_since: Option<Instant>,
    /// Какой стороной к зрителю стоит доска текущей партии.
    orientation: Option<Orientation>,
    /// Ориентацию задал комментатор: новые партии не переворачиваются сами.
    orientation_locked: bool,
}

impl Tracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            stabilizer: Stabilizer::new(&config),
            config,
            game: None,
            lost_since: None,
            orientation: None,
            orientation_locked: false,
        }
    }

    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// Какой стороной к зрителю стоит доска текущей партии. Начиная партию,
    /// трекер мог прочитать доску с другой стороны, чем распознавание (см.
    /// `start`), — тогда распознаванию нужно перейти на эту ориентацию.
    pub fn orientation(&self) -> Option<Orientation> {
        self.orientation
    }

    /// Комментатор сам выбрал, какой стороной стоит доска (или отменил
    /// выбор): пока выбор в силе, новые партии не переворачиваются сами.
    pub fn lock_orientation(&mut self, locked: bool) {
        self.orientation_locked = locked;
    }

    /// Забыть партию: следующая устойчивая расстановка начнёт новую.
    pub fn reset(&mut self) {
        self.game = None;
        self.stabilizer = Stabilizer::new(&self.config);
        self.lost_since = None;
        self.orientation = None;
    }

    /// Комментатор сам задал позицию (исправил распознавание или вставил FEN).
    pub fn set_position(&mut self, position: Chess) -> GameEvent {
        self.game = Some(Game::new(position.clone()));
        self.lost_since = None;
        self.stabilizer.settle();
        GameEvent::Started { position, reason: StartReason::Manual }
    }

    /// Очередной распознанный кадр.
    pub fn observe(&mut self, observation: &Observation, now: Instant) -> Vec<GameEvent> {
        let Some(stable) = self.stabilizer.push(observation, now) else {
            return Vec::new();
        };
        let explanation = match &self.game {
            // Первая устойчивая расстановка начинает партию.
            None => return self.start(&stable, StartReason::First).into_iter().collect(),
            Some(game) => explain(game, &stable, self.config.takeback_plies),
        };
        let Some(explanation) = explanation else {
            return match self.lost_since {
                // «Потерялись» сообщаем один раз, а не на каждом кадре.
                None => {
                    self.lost_since = Some(now);
                    vec![GameEvent::Lost]
                }
                Some(since) if now.duration_since(since) >= self.config.resync_after => {
                    self.start(&stable, StartReason::Resync).into_iter().collect()
                }
                Some(_) => Vec::new(),
            };
        };
        self.lost_since = None;
        self.stabilizer.settle();
        let game = self.game.as_mut().expect("explained against an existing game");
        match explanation {
            Explanation::Unchanged => Vec::new(),
            Explanation::Moves(moves) => {
                let mut events = Vec::with_capacity(moves.len());
                for mv in moves {
                    match game.play(mv) {
                        Ok(ply) => {
                            let (mv, san) = (ply.mv, ply.san);
                            events.push(GameEvent::Moved { ply: game.len(), mv, san });
                        }
                        Err(error) => tracing::warn!(%error, "explained move is not playable"),
                    }
                }
                events
            }
            Explanation::TakeBack(to_ply) => {
                game.truncate(to_ply);
                vec![GameEvent::TookBack { to_ply }]
            }
        }
    }

    fn start(&mut self, observation: &Observation, reason: StartReason) -> Option<GameEvent> {
        // Новая партия может идти с другой стороны: Lichess поворачивает
        // доску к тому, кто играет чёрными. Прочитанная не той стороной, она
        // остаётся допустимой позицией — только белые пешки на седьмой
        // горизонтали, а чёрные на второй, — и анализ шёл бы по бессмыслице.
        // Если фигуры явно стоят вверх ногами, партия начинается с
        // повёрнутой доски.
        let rotated;
        let observation = match observation.evident_orientation() {
            Some(evident) if !self.orientation_locked && evident != observation.orientation => {
                tracing::info!(?evident, "the board is shown from the other side");
                rotated = observation.rotated();
                &rotated
            }
            _ => observation,
        };
        let position = infer::board_to_position(observation, self.game.as_ref().map(Game::current))?;
        self.orientation = Some(observation.orientation);
        self.game = Some(Game::new(position.clone()));
        self.lost_since = None;
        self.stabilizer.settle();
        Some(GameEvent::Started { position, reason })
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(TrackerConfig::default())
    }
}
