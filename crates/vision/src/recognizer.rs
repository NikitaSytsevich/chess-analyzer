use analyzer_chess::{Bitboard, Board, Color, File, Piece, Rank, Role, Square};

use crate::Frame;
use crate::color::{Rgb, median_rgb};
use crate::grid::{Grid, grid_candidates};
use crate::learn::Learner;
use crate::patch::Patch;
use crate::pieces::{PieceSet, bundled_sets, piece_at_index};

/// Имя набора, выученного по самой трансляции.
pub const LEARNED_SET: &str = "выученный";

/// Как доска повёрнута на трансляции.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Orientation {
    #[default]
    WhiteBottom,
    BlackBottom,
}

impl Orientation {
    pub fn flipped(self) -> Self {
        match self {
            Self::WhiteBottom => Self::BlackBottom,
            Self::BlackBottom => Self::WhiteBottom,
        }
    }

    /// Поле шахматной доски в столбце `col` и строке `row` экрана (0 — верх).
    pub fn square(self, col: usize, row: usize) -> Square {
        let (file, rank) = match self {
            Self::WhiteBottom => (col, 7 - row),
            Self::BlackBottom => (7 - col, row),
        };
        Square::from_coords(File::new(file as u32), Rank::new(rank as u32))
    }
}

/// Цвета полей доски.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub light: Rgb,
    pub dark: Rgb,
}

/// Что распознано на одном поле.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    pub piece: Option<Piece>,
    /// `0.0..=1.0`: насколько лучший вариант лучше второго и насколько он
    /// вообще похож на клетку.
    pub confidence: f32,
    /// Поле подсвечено — обычно это последний ход или шах.
    pub highlighted: bool,
}

/// Распознанная доска.
#[derive(Clone, Debug)]
pub struct Observation {
    /// По индексу поля: `a1 = 0 … h8 = 63`.
    pub cells: [Cell; 64],
    pub board: Board,
    pub highlighted: Bitboard,
    pub orientation: Orientation,
    pub mean_confidence: f32,
    pub min_confidence: f32,
    pub set: String,
}

impl Observation {
    pub fn cell(&self, square: Square) -> &Cell {
        &self.cells[usize::from(square)]
    }

    /// Та же доска, прочитанная с другой стороны: каждое поле переходит в
    /// симметричное относительно центра доски (a1 ↔ h8).
    pub fn rotated(&self) -> Observation {
        let mut cells = [Cell::default(); 64];
        let mut board = Board::empty();
        for square in Square::ALL {
            let cell = *self.cell(square);
            cells[usize::from(square.rotate_180())] = cell;
            if let Some(piece) = cell.piece {
                board.set_piece_at(square.rotate_180(), piece);
            }
        }
        Observation {
            cells,
            board,
            highlighted: self.highlighted.rotate_180(),
            orientation: self.orientation.flipped(),
            ..self.clone()
        }
    }

    /// Какой стороной к зрителю стоит доска, судя по самим фигурам, — если
    /// это очевидно (см. `white_bottom_balance`). `None`, когда фигуры
    /// ничего определённого не говорят: в эндшпиле короли и редкие пешки
    /// бывают где угодно.
    pub fn evident_orientation(&self) -> Option<Orientation> {
        let pieces = Square::ALL.into_iter().filter_map(|square| {
            let cell = self.cell(square);
            // Строка экрана, где стоит поле: 0 — верх.
            let row = match self.orientation {
                Orientation::WhiteBottom => 7 - square.rank() as usize,
                Orientation::BlackBottom => square.rank() as usize,
            };
            Some((cell.piece?, cell.confidence, row))
        });
        let balance = white_bottom_balance(pieces);
        (balance.abs() >= EVIDENT).then_some(if balance < 0.0 {
            Orientation::BlackBottom
        } else {
            Orientation::WhiteBottom
        })
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum VisionError {
    #[error("доска не найдена")]
    NoBoard,
}

/// Клетки кадра в порядке экрана: строка за строкой сверху вниз.
struct Scan {
    patches: Vec<Patch>,
    backgrounds: Vec<Rgb>,
}

impl Scan {
    fn is_light(index: usize) -> bool {
        // Левый верхний угол доски — всегда светлое поле: a8 у белых снизу,
        // h1 у чёрных снизу.
        (index / 8 + index % 8).is_multiple_of(2)
    }
}

/// Отличие клетки от ровного поля, после которого на ней что-то стоит.
const OCCUPIED: f32 = 12.0;
/// Фон клетки отклонился от цвета поля настолько — клетка подсвечена.
const HIGHLIGHT: f32 = 14.0;

/// Распознаватель одной трансляции: помнит положение доски, её цвета,
/// ориентацию и набор фигур между кадрами.
pub struct Recognizer {
    sets: Vec<PieceSet>,
    active: Option<usize>,
    learner: Learner,
    grid: Option<Grid>,
    palette: Option<Palette>,
    orientation: Option<Orientation>,
}

impl Default for Recognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Recognizer {
    pub fn new() -> Self {
        Self::with_sets(bundled_sets())
    }

    pub fn with_sets(sets: Vec<PieceSet>) -> Self {
        assert!(!sets.is_empty(), "a recognizer needs at least one piece set");
        Self { sets, active: None, learner: Learner::default(), grid: None, palette: None, orientation: None }
    }

    pub fn grid(&self) -> Option<Grid> {
        self.grid
    }

    pub fn palette(&self) -> Option<Palette> {
        self.palette
    }

    pub fn orientation(&self) -> Option<Orientation> {
        self.orientation
    }

    pub fn set_orientation(&mut self, orientation: Orientation) {
        self.orientation = Some(orientation);
    }

    pub fn active_set(&self) -> Option<&str> {
        self.active.map(|index| self.sets[index].name.as_str())
    }

    /// Забыть положение доски: раскладка трансляции сменилась, ищем заново.
    pub fn forget_grid(&mut self) {
        self.grid = None;
        self.palette = None;
    }

    /// Забыть ориентацию: на другой трансляции доска может стоять другой
    /// стороной, и первый же кадр определит её заново.
    pub fn forget_orientation(&mut self) {
        self.orientation = None;
    }

    /// Ищет доску на кадре и запоминает её положение и цвета.
    pub fn locate(&mut self, frame: &Frame) -> Result<Grid, VisionError> {
        let (grid, palette) = best_board(frame, 0.45).ok_or(VisionError::NoBoard)?;
        self.grid = Some(grid);
        self.palette = Some(palette);
        Ok(grid)
    }

    /// Распознаёт доску на кадре. Если положение доски ещё неизвестно,
    /// сначала ищет её.
    pub fn observe(&mut self, frame: &Frame) -> Result<Observation, VisionError> {
        let grid = match self.grid {
            Some(grid) => grid,
            None => self.locate(frame)?,
        };
        let palette = self.palette.ok_or(VisionError::NoBoard)?;
        let scan = scan(frame, &grid);
        // Доска ещё на месте? Страница могла смениться другой — ошибкой сети,
        // заставкой, — и её ровные клетки читались бы уверенно, как пустая
        // доска: стрелки поверх трансляции повисли бы над пустым местом.
        if matching_cells(&scan, palette) < STILL_BOARD {
            return Err(VisionError::NoBoard);
        }

        if self.active.is_none() {
            self.choose_set(&scan);
            // Незнакомые фигуры, но на доске начальная позиция: учим набор
            // прямо по ней — так Chess.com узнаётся с первой же партии.
            if self.mean_confidence(&scan, self.active.unwrap_or(0)) < 0.6
                && let Some(orientation) = initial_position_orientation(&scan)
            {
                self.orientation = Some(orientation);
                self.learn_scan(&scan, &Board::default(), orientation);
            }
        }
        let active = self.active.unwrap_or(0);
        let classified: Vec<(Option<Piece>, f32)> =
            (0..64).map(|i| classify(&scan.patches[i], scan.backgrounds[i], &self.sets[active])).collect();
        let orientation = *self.orientation.get_or_insert_with(|| infer_orientation(&classified));

        let mut cells = [Cell::default(); 64];
        let mut board = Board::empty();
        let mut highlighted = Bitboard::EMPTY;
        for (index, (piece, confidence)) in classified.iter().enumerate() {
            let square = orientation.square(index % 8, index / 8);
            let base = if Scan::is_light(index) { palette.light } else { palette.dark };
            let lit = scan.backgrounds[index].distance(base) >= HIGHLIGHT;
            cells[usize::from(square)] = Cell { piece: *piece, confidence: *confidence, highlighted: lit };
            if let Some(piece) = piece {
                board.set_piece_at(square, *piece);
            }
            if lit {
                highlighted |= Bitboard::from(square);
            }
        }
        let mean_confidence = cells.iter().map(|c| c.confidence).sum::<f32>() / 64.0;
        let min_confidence = cells.iter().map(|c| c.confidence).fold(1.0, f32::min);
        Ok(Observation {
            cells,
            board,
            highlighted,
            orientation,
            mean_confidence,
            min_confidence,
            set: self.sets[active].name.clone(),
        })
    }

    /// Доучивает набор фигур по кадру с известной расстановкой: её
    /// подтвердил комментатор или правила игры (ход вывел ровно сюда).
    pub fn learn(&mut self, frame: &Frame, board: &Board) -> Result<(), VisionError> {
        let grid = self.grid.ok_or(VisionError::NoBoard)?;
        let orientation = self.orientation.unwrap_or_default();
        let scan = scan(frame, &grid);
        self.learn_scan(&scan, board, orientation);
        Ok(())
    }

    fn learn_scan(&mut self, scan: &Scan, board: &Board, orientation: Orientation) {
        for index in 0..64 {
            let square = orientation.square(index % 8, index / 8);
            if let Some(piece) = board.piece_at(square) {
                self.learner.add(piece, Scan::is_light(index), &scan.patches[index], scan.backgrounds[index]);
            }
        }
        let fallback = &self.sets[self.active.unwrap_or(0)];
        let learned = self.learner.build(LEARNED_SET, fallback);
        let index = match self.sets.iter().position(|set| set.name == LEARNED_SET) {
            Some(index) => {
                self.sets[index] = learned;
                index
            }
            None => {
                self.sets.push(learned);
                self.sets.len() - 1
            }
        };
        self.active = Some(index);
    }

    /// Выбирает набор фигур, лучше всего объясняющий кадр.
    fn choose_set(&mut self, scan: &Scan) {
        let best = (0..self.sets.len())
            .map(|index| (index, self.mean_confidence(scan, index)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        self.active = best.map(|(index, _)| index);
    }

    /// Средняя уверенность по занятым клеткам: пустые клетки одинаково
    /// хорошо объясняет любой набор, и они только размывали бы выбор.
    fn mean_confidence(&self, scan: &Scan, set: usize) -> f32 {
        let mut sum = 0.0;
        let mut count = 0.0;
        for i in 0..64 {
            if scan.patches[i].distance_to_flat(scan.backgrounds[i]) < OCCUPIED {
                continue;
            }
            sum += classify(&scan.patches[i], scan.backgrounds[i], &self.sets[set]).1;
            count += 1.0;
        }
        if count == 0.0 { 0.0 } else { sum / count }
    }
}

/// Цвета полей, если на месте `grid` действительно доска: поля двух
/// заметно разных цветов, и почти каждая клетка своего цвета (кроме пары
/// подсвеченных).
fn palette_for(frame: &Frame, grid: &Grid) -> Option<Palette> {
    let scan = scan(frame, grid);
    let parity = |light: bool| {
        median_rgb(
            &(0..64).filter(|&i| Scan::is_light(i) == light).map(|i| scan.backgrounds[i]).collect::<Vec<_>>(),
        )
    };
    let palette = Palette { light: parity(true), dark: parity(false) };
    // Подсвечены бывают последний ход, шах и предварительный ход — до пяти
    // клеток. Сетка, сдвинутая на клетку, захватывает целый ряд страницы —
    // восемь чужих клеток — и здесь отсеивается.
    (palette.light.distance(palette.dark) >= 18.0 && matching_cells(&scan, palette) >= 57).then_some(palette)
}

/// Столько клеток найденной доски должны быть цвета своего поля на каждом
/// кадре, иначе на её месте уже не доска. Найти доску строже (57 из 64):
/// здесь запас на выделенные комментатором клетки и сжатие видео.
const STILL_BOARD: usize = 48;

/// Сколько клеток цвета своего поля.
fn matching_cells(scan: &Scan, palette: Palette) -> usize {
    (0..64)
        .filter(|&i| {
            let base = if Scan::is_light(i) { palette.light } else { palette.dark };
            scan.backgrounds[i].distance(base) < HIGHLIGHT
        })
        .count()
}

/// Первый кандидат сетки, который действительно доска.
fn best_board(frame: &Frame, min_fraction: f32) -> Option<(Grid, Palette)> {
    grid_candidates(frame, min_fraction)
        .into_iter()
        .filter(|(_, quality)| *quality >= 2.0)
        .find_map(|(grid, _)| palette_for(frame, &grid).map(|palette| (grid, palette)))
}

/// Доска где-то в окне трансляции — для первичной настройки, когда захвачено
/// всё окно браузера, а доска занимает его малую часть.
pub fn find_board(frame: &Frame) -> Option<(Grid, Palette)> {
    best_board(frame, 0.12)
}

fn scan(frame: &Frame, grid: &Grid) -> Scan {
    let mut patches = Vec::with_capacity(64);
    let mut backgrounds = Vec::with_capacity(64);
    for row in 0..8 {
        for col in 0..8 {
            let (x, y) = grid.cell_origin(col, row);
            let patch = Patch::sample(frame, x, y, grid.square);
            backgrounds.push(patch.background());
            patches.push(patch);
        }
    }
    Scan { patches, backgrounds }
}

/// Уверенность полная, когда лучший вариант ближе второго на столько.
const FULL_MARGIN: f32 = 10.0;
/// Расхождение с лучшим шаблоном, до которого совпадение считается хорошим…
const GOOD_MATCH: f32 = 10.0;
/// …и после которого клетка не похожа ни на что (стрелка, курсор, помехи).
const NO_MATCH: f32 = 40.0;

/// Лучший вариант для клетки — фигура или пусто — и уверенность в нём.
fn classify(patch: &Patch, background: Rgb, set: &PieceSet) -> (Option<Piece>, f32) {
    let mut best = (None, patch.distance_to_flat(background));
    let mut second = f32::INFINITY;
    for (index, template) in set.templates.iter().enumerate() {
        let distance = template.distance(patch, background);
        if distance < best.1 {
            second = best.1;
            best = (Some(piece_at_index(index)), distance);
        } else if distance < second {
            second = distance;
        }
    }
    let margin = ((second - best.1) / FULL_MARGIN).clamp(0.0, 1.0);
    let quality = (1.0 - (best.1 - GOOD_MATCH) / (NO_MATCH - GOOD_MATCH)).clamp(0.0, 1.0);
    (best.0, margin * quality)
}

/// Ориентация по распознанным фигурам (см. [`white_bottom_balance`]). Если
/// не на что опереться — белые снизу.
fn infer_orientation(classified: &[(Option<Piece>, f32)]) -> Orientation {
    let pieces = classified
        .iter()
        .enumerate()
        .filter_map(|(index, (piece, confidence))| Some(((*piece)?, *confidence, index / 8)));
    if white_bottom_balance(pieces) < 0.0 { Orientation::BlackBottom } else { Orientation::WhiteBottom }
}

/// С этого перевеса ориентация по фигурам считается очевидной. У начальной
/// позиции он около 60, в миттельшпиле — десятки, а в эндшпиле с
/// королями в центре — единицы.
const EVIDENT: f32 = 6.0;

/// Насколько расстановка похожа на «белые снизу»: белые пешки и король
/// живут ближе к своей стороне доски, чёрные — к своей. Больше нуля — белые
/// снизу, меньше — сверху. Фигуры — с уверенностью распознавания и строкой
/// экрана (0 — верх).
fn white_bottom_balance(pieces: impl Iterator<Item = (Piece, f32, usize)>) -> f32 {
    pieces
        .map(|(piece, confidence, row)| {
            let weight = match piece.role {
                Role::Pawn => 1.0,
                Role::King => 3.0,
                _ => 0.0,
            } * confidence;
            // Строка экрана от −3.5 (верх) до +3.5 (низ).
            let row = row as f32 - 3.5;
            match piece.color {
                Color::White => weight * row,
                Color::Black => -weight * row,
            }
        })
        .sum()
}

/// Начальная позиция узнаётся без шаблонов: заняты ровно две верхние и две
/// нижние горизонтали. Какая сторона белая — по яркости фигур.
fn initial_position_orientation(scan: &Scan) -> Option<Orientation> {
    let occupied = |i: usize| scan.patches[i].distance_to_flat(scan.backgrounds[i]) >= OCCUPIED;
    for index in 0..64 {
        let row = index / 8;
        let should = row <= 1 || row >= 6;
        if occupied(index) != should {
            return None;
        }
    }
    // Средняя яркость «чернил» — ячеек, отличных от фона, — сверху и снизу.
    let ink = |rows: std::ops::Range<usize>| {
        let mut sum = 0.0;
        let mut count = 0.0;
        for index in rows.flat_map(|row| row * 8..row * 8 + 8) {
            let background = scan.backgrounds[index];
            for cell in &scan.patches[index].cells {
                if cell.distance(background) > 20.0 {
                    sum += cell.luma();
                    count += 1.0;
                }
            }
        }
        if count == 0.0 { 0.0 } else { sum / count }
    };
    let top = ink(0..2);
    let bottom = ink(6..8);
    if (top - bottom).abs() < 0.08 {
        return None;
    }
    Some(if bottom > top { Orientation::WhiteBottom } else { Orientation::BlackBottom })
}

#[cfg(test)]
mod tests {
    use analyzer_chess::{Chess, Fen, Position};

    use super::*;

    /// Уверенно распознанная доска, прочитанная как «белые снизу».
    fn observation(board: &Board) -> Observation {
        let mut cells = [Cell { piece: None, confidence: 1.0, highlighted: false }; 64];
        for square in Square::ALL {
            cells[usize::from(square)].piece = board.piece_at(square);
        }
        Observation {
            cells,
            board: board.clone(),
            highlighted: Bitboard::from(Square::E2),
            orientation: Orientation::WhiteBottom,
            mean_confidence: 1.0,
            min_confidence: 1.0,
            set: "test".into(),
        }
    }

    #[test]
    fn a_rotated_board_is_read_from_the_other_side() {
        let start = observation(Chess::default().board());
        let rotated = start.rotated();
        assert_eq!(rotated.orientation, Orientation::BlackBottom);
        // Белая ладья a1 оказалась на h8, подсветка e2 — на d7.
        assert_eq!(rotated.board.piece_at(Square::H8), start.board.piece_at(Square::A1));
        assert_eq!(rotated.cell(Square::H8).piece, start.board.piece_at(Square::A1));
        assert_eq!(rotated.highlighted, Bitboard::from(Square::D7));
        assert_eq!(rotated.rotated().board, start.board);
    }

    #[test]
    fn pieces_tell_which_side_is_at_the_bottom() {
        let start = observation(Chess::default().board());
        assert_eq!(start.evident_orientation(), Some(Orientation::WhiteBottom));
        // Партия с чёрными снизу, прочитанная как «белые снизу»: белые пешки
        // на седьмой горизонтали, чёрные — на второй.
        let upside_down = Observation { orientation: Orientation::WhiteBottom, ..start.rotated() };
        assert_eq!(upside_down.evident_orientation(), Some(Orientation::BlackBottom));
        // Ответ — про экран: прочитай ту же доску с другой стороны, белые на
        // экране всё равно снизу.
        assert_eq!(start.rotated().evident_orientation(), Some(Orientation::WhiteBottom));
    }

    #[test]
    fn kings_in_the_centre_say_nothing() {
        let fen: Fen = "8/8/8/3k4/4K3/8/8/8 w - - 0 1".parse().unwrap();
        let endgame = observation(&fen.as_setup().board);
        assert_eq!(endgame.evident_orientation(), None);
        assert_eq!(endgame.rotated().evident_orientation(), None);
    }
}
