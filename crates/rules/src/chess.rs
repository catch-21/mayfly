//! `chess/1` — two players, one legal move at a time (§10.2).
//!
//! Legality, checkmate, stalemate, insufficient material, FEN, SAN and UCI come from
//! [shakmaty](https://crates.io/crates/shakmaty). The rules do not read `link.ts` or a receipt:
//! a time is not part of the position. Threefold repetition and the fifty-move rule end the
//! game as a draw on the move that produces them, because the kind list has no claim.
//!
//! Colour is fixed when both seats carry `role` (`white` or `black`). When neither does,
//! `wants_reveals` is set and the low bit of `BLAKE3(nonces concatenated in party order)`
//! chooses: `0` means `parties[0]` is White. The fold passes the decoded genesis nonce as
//! `nonces[0]` and each other party's revealed nonce after it (§6.6).

use serde::{Deserialize, Serialize};
use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{
    CastlingMode, Chess as ChessPos, Color, EnPassantMode, KnownOutcome, Move, Outcome, Position,
    Role,
};

use pubky_mayfly::genesis::Genesis;
use pubky_mayfly::hash::Hash;
use pubky_mayfly::record::{CloseBody, CloseReason, Confirmation, Link};
use pubky_mayfly::rules::{Nonce, Outcome as RulesOutcome, PartyIndex, Rules, RulesError, Status};

/// The rules id.
pub const ID: &str = "chess/1";

/// Placeholder until the published module is hashed (§6.6).
pub const REFERENCE_HASH: &str = "chess/1-reference-hash-placeholder";

/// Game state. The position is the initial FEN plus `moves`, replayed through shakmaty.
/// `result` is set once the rules have ended the game, including a resignation, which the
/// position alone cannot show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Canonical FEN of the position before any move.
    pub initial_fen: String,
    /// UCI of each move, in order.
    pub moves: Vec<String>,
    /// Party index of White.
    pub white: PartyIndex,
    /// Party index of Black.
    pub black: PartyIndex,
    /// Pubkys in genesis order, so a link author can be seated.
    pub parties: Vec<String>,
    /// Party who has a draw offer open, if any.
    pub draw_offer: Option<PartyIndex>,
    /// `1-0`, `0-1` or `1/2-1/2` once the game has ended.
    pub result: Option<String>,
}

/// Link body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// A move. `san`, when present, must be that move's SAN.
    Move {
        /// UCI, such as `e2e4` or `e7e8q`.
        uci: String,
        /// SAN, such as `e4` or `O-O`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        san: Option<String>,
    },
    /// The author resigns.
    Resign {},
    /// The author offers a draw. A later move declines it.
    OfferDraw {},
    /// The other party accepts the open offer.
    AcceptDraw {},
}

/// What a board renders. Derived from [`State`]; not hashed into the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChessView {
    /// FEN of the current position.
    pub fen: String,
    /// `"white"` or `"black"`.
    pub turn: String,
    /// Party to move, absent once the game has ended.
    pub turn_party: Option<PartyIndex>,
    /// Whether the king of the side to move is in check.
    pub in_check: bool,
    /// Legal UCI moves, sorted. Empty once the game has ended.
    pub legal_uci: Vec<String>,
    /// The result, once the game has ended.
    pub result: Option<String>,
    /// SAN of each played move, with check and mate marks.
    pub san: Vec<String>,
    /// Pieces White has taken, most valuable first (`Q`, `R`, `B`, `N`, `P`).
    pub captured_by_white: String,
    /// Pieces Black has taken, in the same order.
    pub captured_by_black: String,
}

/// The `chess/1` rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct Chess;

impl Rules for Chess {
    type State = State;
    type Body = Body;

    fn id(&self) -> &'static str {
        ID
    }

    fn reference_hash(&self) -> &'static str {
        REFERENCE_HASH
    }

    fn init(
        &self,
        genesis: &Genesis,
        _confirmations: &[Confirmation],
        nonces: &[Nonce],
    ) -> Result<State, RulesError> {
        if genesis.parties.len() != 2 {
            return Err(RulesError("chess/1 has two parties".into()));
        }
        if genesis.confirm_quorum != 2 {
            return Err(RulesError("chess/1 confirm_quorum is 2".into()));
        }
        let (white, black) = seats(genesis, nonces)?;
        let pos = setup(genesis)?;
        let mut state = State {
            initial_fen: canonical_fen(&pos),
            moves: Vec::new(),
            white,
            black,
            parties: genesis.parties.iter().map(|p| p.pubky.clone()).collect(),
            draw_offer: None,
            result: None,
        };
        let (pos, history) = replay(&state)?;
        state.result = terminal(&pos, &history);
        Ok(state)
    }

    fn wants_reveals(&self, genesis: &Genesis) -> bool {
        genesis.parties.len() == 2 && genesis.parties.iter().all(|p| p.role.is_none())
    }

    fn obliged(&self, state: &State) -> Vec<PartyIndex> {
        side_to_move(state).into_iter().collect()
    }

    fn may_append(&self, state: &State, party: PartyIndex, kind: &str) -> bool {
        eligible(state, party, kind)
    }

    fn apply(&self, state: &State, link: &Link) -> Result<State, RulesError> {
        if state.result.is_some() {
            return Err(RulesError("the game is over".into()));
        }
        let author = author_index(state, link)?;
        let body = decode_body(link)?;
        let kind = link.kind.as_str();
        if !eligible(state, author, kind) {
            return Err(RulesError(format!("party {author} may not append {kind}")));
        }
        let mut next = state.clone();
        match body {
            Body::Move { uci, san } => play_move(&mut next, &uci, san.as_deref())?,
            Body::Resign {} => {
                next.draw_offer = None;
                next.result = Some(loss_for(state, author).to_string());
            }
            Body::OfferDraw {} => next.draw_offer = Some(author),
            Body::AcceptDraw {} => {
                next.draw_offer = None;
                next.result = Some("1/2-1/2".into());
            }
        }
        Ok(next)
    }

    fn status(&self, state: &State) -> Status {
        match &state.result {
            Some(summary) => Status::Finished(RulesOutcome {
                summary: summary.clone(),
                winners: winners(state, summary),
            }),
            None => Status::Ongoing,
        }
    }

    fn close(&self, state: &State, close: &CloseBody) -> Result<RulesOutcome, RulesError> {
        match close.reason {
            CloseReason::Finished => {
                let summary = state
                    .result
                    .clone()
                    .ok_or_else(|| RulesError("the game is not over".into()))?;
                Ok(RulesOutcome {
                    winners: winners(state, &summary),
                    summary,
                })
            }
            CloseReason::Agreed => Ok(RulesOutcome {
                summary: "1/2-1/2".into(),
                winners: Vec::new(),
            }),
            CloseReason::Abandoned => {
                if close.subject.len() != 1 {
                    return Err(RulesError(
                        "an abandoned chess close names one player".into(),
                    ));
                }
                let loser = state
                    .parties
                    .iter()
                    .position(|p| p == &close.subject[0])
                    .ok_or_else(|| RulesError("abandoned subject is not seated".into()))?;
                let summary = loss_for(state, loser).to_string();
                Ok(RulesOutcome {
                    winners: winners(state, &summary),
                    summary,
                })
            }
        }
    }

    fn canonical_state(&self, state: &State) -> Vec<u8> {
        serde_json::to_vec(state).expect("state serialises")
    }
}

/// The board view of `state`.
pub fn chess_view(state: &State) -> Result<ChessView, RulesError> {
    let (pos, _) = replay(state)?;
    let over = state.result.is_some();
    let mut legal_uci: Vec<String> = if over {
        Vec::new()
    } else {
        pos.legal_moves().iter().map(uci_of).collect()
    };
    legal_uci.sort();
    let (captured_by_white, captured_by_black) = captured(state)?;
    Ok(ChessView {
        fen: canonical_fen(&pos),
        turn: colour_name(pos.turn()).into(),
        turn_party: if over { None } else { side_to_move(state) },
        in_check: pos.is_check(),
        legal_uci,
        result: state.result.clone(),
        san: sans(state)?,
        captured_by_white,
        captured_by_black,
    })
}

/// PGN movetext. `think_ms[i]` is the witnessed think time of move `i`, written as `{ m:ss }`.
/// A missing or absent entry leaves the move without a comment. This is how long the move
/// took, not time remaining.
pub fn pgn(state: &State, think_ms: &[Option<u64>]) -> Result<String, RulesError> {
    let sans = sans(state)?;
    let (start, _) = replay_until(state, 0)?;
    let mut full = start.fullmoves().get();
    let mut white_to_move = start.turn() == Color::White;
    let mut text = String::new();
    for (i, san) in sans.iter().enumerate() {
        if white_to_move {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("{full}. {san}"));
        } else {
            if text.is_empty() {
                text.push_str(&format!("{full}... {san}"));
            } else {
                text.push(' ');
                text.push_str(san);
            }
            full += 1;
        }
        if let Some(Some(ms)) = think_ms.get(i) {
            text.push_str(&format!(" {{ {} }}", clock(*ms)));
        }
        white_to_move = !white_to_move;
    }
    let result = state.result.as_deref().unwrap_or("*");
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(result);
    let white = &state.parties[state.white];
    let black = &state.parties[state.black];
    Ok(format!(
        "[Event \"Mayfly chess/1\"]\n[Site \"pubky\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n[Result \"{result}\"]\n\n{text}\n"
    ))
}

fn seats(genesis: &Genesis, nonces: &[Nonce]) -> Result<(PartyIndex, PartyIndex), RulesError> {
    let role = |i: usize| genesis.parties[i].role.as_deref();
    match (role(0), role(1)) {
        (None, None) => {
            if nonces.len() != 2 {
                return Err(RulesError("colour needs both nonces".into()));
            }
            let mut bytes = Vec::new();
            for nonce in nonces {
                bytes.extend_from_slice(nonce);
            }
            let white = usize::from(Hash::of(&bytes).as_bytes()[0] & 1);
            Ok((white, 1 - white))
        }
        (Some("white"), Some("black")) => Ok((0, 1)),
        (Some("black"), Some("white")) => Ok((1, 0)),
        (None, Some(_)) | (Some(_), None) => {
            Err(RulesError("both seats need a colour, or neither".into()))
        }
        _ => Err(RulesError("roles are white and black".into())),
    }
}

fn setup(genesis: &Genesis) -> Result<ChessPos, RulesError> {
    let fen = match genesis.options.get("initial_fen") {
        None | Some(serde_json::Value::Null) => return Ok(ChessPos::default()),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(_) => return Err(RulesError("initial_fen is a string".into())),
    };
    let parsed: Fen = fen
        .parse()
        .map_err(|e| RulesError(format!("initial_fen: {e}")))?;
    parsed
        .into_position(CastlingMode::Standard)
        .map_err(|e| RulesError(format!("initial_fen: {e}")))
}

fn canonical_fen(pos: &ChessPos) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

fn replay(state: &State) -> Result<(ChessPos, Vec<ChessPos>), RulesError> {
    replay_until(state, state.moves.len())
}

/// The position after `n` moves, and every position from the start through that one.
fn replay_until(state: &State, n: usize) -> Result<(ChessPos, Vec<ChessPos>), RulesError> {
    let mut pos = Fen::from_ascii(state.initial_fen.as_bytes())
        .map_err(|e| RulesError(format!("stored fen: {e}")))?
        .into_position::<ChessPos>(CastlingMode::Standard)
        .map_err(|e| RulesError(format!("stored fen: {e}")))?;
    let mut history = vec![pos.clone()];
    for uci in state.moves.iter().take(n) {
        let mv = parse_uci(&pos, uci)?;
        pos = pos
            .play(mv)
            .map_err(|_| RulesError(format!("stored move {uci} is illegal")))?;
        history.push(pos.clone());
    }
    Ok((pos, history))
}

fn captured(state: &State) -> Result<(String, String), RulesError> {
    let mut pos = Fen::from_ascii(state.initial_fen.as_bytes())
        .map_err(|e| RulesError(format!("stored fen: {e}")))?
        .into_position::<ChessPos>(CastlingMode::Standard)
        .map_err(|e| RulesError(format!("stored fen: {e}")))?;
    let mut white = Vec::new();
    let mut black = Vec::new();
    for uci in &state.moves {
        let mv = parse_uci(&pos, uci)?;
        if let Some(role) = capture_role(mv) {
            if pos.turn() == Color::White {
                white.push(role);
            } else {
                black.push(role);
            }
        }
        pos = pos
            .play(mv)
            .map_err(|_| RulesError(format!("stored move {uci} is illegal")))?;
    }
    Ok((pieces(white), pieces(black)))
}

fn capture_role(mv: Move) -> Option<Role> {
    match mv {
        Move::Normal { capture, .. } => capture,
        Move::EnPassant { .. } => Some(Role::Pawn),
        Move::Castle { .. } | Move::Put { .. } => None,
    }
}

fn pieces(mut roles: Vec<Role>) -> String {
    roles.sort_by_key(|role| std::cmp::Reverse(piece_rank(*role)));
    roles.into_iter().map(piece_letter).collect()
}

fn piece_rank(role: Role) -> u8 {
    match role {
        Role::Queen => 5,
        Role::Rook => 4,
        Role::Bishop => 3,
        Role::Knight => 2,
        Role::Pawn => 1,
        Role::King => 0,
    }
}

fn piece_letter(role: Role) -> char {
    match role {
        Role::Queen => 'Q',
        Role::Rook => 'R',
        Role::Bishop => 'B',
        Role::Knight => 'N',
        Role::Pawn => 'P',
        Role::King => 'K',
    }
}

fn sans(state: &State) -> Result<Vec<String>, RulesError> {
    let mut pos = Fen::from_ascii(state.initial_fen.as_bytes())
        .map_err(|e| RulesError(format!("stored fen: {e}")))?
        .into_position::<ChessPos>(CastlingMode::Standard)
        .map_err(|e| RulesError(format!("stored fen: {e}")))?;
    let mut out = Vec::with_capacity(state.moves.len());
    for uci in &state.moves {
        let mv = parse_uci(&pos, uci)?;
        out.push(san_of(&pos, mv));
        pos = pos
            .play(mv)
            .map_err(|_| RulesError(format!("stored move {uci} is illegal")))?;
    }
    Ok(out)
}

fn play_move(state: &mut State, uci: &str, san: Option<&str>) -> Result<(), RulesError> {
    let (pos, history) = replay(state)?;
    let mv = parse_uci(&pos, uci)?;
    let written = san_of(&pos, mv);
    if let Some(claimed) = san {
        if claimed != written {
            return Err(RulesError(format!("san is {written}, not {claimed}")));
        }
    }
    let pos = pos
        .play(mv)
        .map_err(|_| RulesError(format!("{uci} is illegal")))?;
    let mut history = history;
    history.push(pos.clone());
    state.moves.push(uci_of(&mv));
    state.draw_offer = None;
    state.result = terminal(&pos, &history);
    Ok(())
}

fn terminal(pos: &ChessPos, history: &[ChessPos]) -> Option<String> {
    match pos.outcome() {
        Outcome::Known(KnownOutcome::Decisive { winner }) => Some(score(winner).into()),
        Outcome::Known(KnownOutcome::Draw) => Some("1/2-1/2".into()),
        Outcome::Unknown
            if pos.halfmoves() >= 100 || history.iter().filter(|p| *p == pos).count() >= 3 =>
        {
            Some("1/2-1/2".into())
        }
        Outcome::Unknown => None,
    }
}

fn parse_uci(pos: &ChessPos, uci: &str) -> Result<Move, RulesError> {
    let parsed: UciMove = uci.parse().map_err(|e| RulesError(format!("uci: {e}")))?;
    parsed
        .to_move(pos)
        .map_err(|_| RulesError(format!("{uci} is illegal")))
}

fn uci_of(mv: &Move) -> String {
    UciMove::from_standard(*mv).to_string()
}

fn san_of(pos: &ChessPos, mv: Move) -> String {
    SanPlus::from_move(pos.clone(), mv).to_string()
}

fn side_to_move(state: &State) -> Option<PartyIndex> {
    if state.result.is_some() {
        return None;
    }
    let (pos, _) = replay(state).ok()?;
    Some(seat_of(state, pos.turn()))
}

fn eligible(state: &State, party: PartyIndex, kind: &str) -> bool {
    if state.result.is_some() || party >= state.parties.len() {
        return false;
    }
    let Some(turn) = side_to_move(state) else {
        return false;
    };
    match kind {
        "move" => party == turn,
        "resign" => true,
        "offer_draw" => state.draw_offer.is_none(),
        "accept_draw" => state.draw_offer.is_some_and(|offerer| offerer != party),
        _ => false,
    }
}

fn author_index(state: &State, link: &Link) -> Result<PartyIndex, RulesError> {
    state
        .parties
        .iter()
        .position(|p| p == &link.author)
        .ok_or_else(|| RulesError("author is not seated".into()))
}

fn decode_body(link: &Link) -> Result<Body, RulesError> {
    let mut body = link.body.clone();
    let Some(obj) = body.as_object_mut() else {
        return Err(RulesError("body is an object".into()));
    };
    obj.insert("kind".into(), serde_json::Value::String(link.kind.clone()));
    serde_json::from_value(body).map_err(|e| RulesError(format!("body: {e}")))
}

fn seat_of(state: &State, color: Color) -> PartyIndex {
    match color {
        Color::White => state.white,
        Color::Black => state.black,
    }
}

fn score(winner: Color) -> &'static str {
    match winner {
        Color::White => "1-0",
        Color::Black => "0-1",
    }
}

fn loss_for(state: &State, loser: PartyIndex) -> &'static str {
    if loser == state.white {
        "0-1"
    } else {
        "1-0"
    }
}

fn winners(state: &State, summary: &str) -> Vec<PartyIndex> {
    match summary {
        "1-0" => vec![state.white],
        "0-1" => vec![state.black],
        _ => Vec::new(),
    }
}

fn colour_name(color: Color) -> &'static str {
    match color {
        Color::White => "white",
        Color::Black => "black",
    }
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pubky_mayfly::genesis::{Party, Witness};
    use pubky_mayfly::hash::ChainId;
    use pubky_mayfly::record::Link;
    use pubky_mayfly::PROTOCOL_VERSION;
    use serde_json::json;

    fn party(pubky: &str, role: Option<&str>) -> Party {
        Party {
            pubky: pubky.into(),
            kid: None,
            role: role.map(str::to_string),
            path: None,
        }
    }

    fn genesis(roles: [Option<&str>; 2], quorum: u32, fen: Option<&str>) -> Genesis {
        let mut options = json!({});
        if let Some(fen) = fen {
            options["initial_fen"] = json!(fen);
        }
        Genesis {
            rules: ID.into(),
            rules_hash: REFERENCE_HASH.into(),
            max_body_bytes: 65_536,
            parties: vec![party("alice", roles[0]), party("bob", roles[1])],
            nonce: "bm9uY2U".into(),
            confirm_quorum: quorum,
            witnesses: Vec::<Witness>::new(),
            recovery_delay_ms: 86_400_000,
            options,
        }
    }

    fn start() -> State {
        Chess
            .init(&genesis([Some("white"), Some("black")], 2, None), &[], &[])
            .unwrap()
    }

    fn play(state: &State, author: &str, body: Body) -> Result<State, RulesError> {
        let mut value = serde_json::to_value(&body).unwrap();
        let kind = value
            .as_object_mut()
            .unwrap()
            .remove("kind")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let link = Link {
            v: PROTOCOL_VERSION,
            chain: ChainId::none(),
            seq: 1,
            round: 0,
            prev: String::new(),
            confirms: Vec::new(),
            receipts: Vec::new(),
            author: author.into(),
            kid: String::new(),
            ts: 0,
            kind,
            body: value,
            state: String::new(),
            grant: None,
        };
        Chess.apply(state, &link)
    }

    fn moves(state: &State, ucis: &[(&str, &str)]) -> State {
        let mut state = state.clone();
        for (author, uci) in ucis {
            state = play(
                &state,
                author,
                Body::Move {
                    uci: (*uci).into(),
                    san: None,
                },
            )
            .unwrap_or_else(|e| panic!("{uci}: {e}"));
        }
        state
    }

    fn close_body(reason: CloseReason, subject: &[&str]) -> CloseBody {
        CloseBody {
            reason,
            subject: subject.iter().map(|s| (*s).to_string()).collect(),
            pending: Vec::new(),
        }
    }

    #[test]
    fn scholars_mate_and_the_pgn() {
        let state = moves(
            &start(),
            &[
                ("alice", "e2e4"),
                ("bob", "e7e5"),
                ("alice", "d1h5"),
                ("bob", "b8c6"),
                ("alice", "f1c4"),
                ("bob", "g8f6"),
                ("alice", "h5f7"),
            ],
        );
        assert_eq!(state.result.as_deref(), Some("1-0"));
        assert!(matches!(Chess.status(&state), Status::Finished(_)));
        assert!(!Chess.may_append(&state, 0, "move"));
        let sans = sans(&state).unwrap();
        assert_eq!(sans, ["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7#"]);
        let view = chess_view(&state).unwrap();
        assert_eq!(view.captured_by_white, "P");
        assert_eq!(view.captured_by_black, "");
        let pgn = pgn(&state, &[Some(72_000), None, Some(3_000)]).unwrap();
        assert!(pgn.contains("[White \"alice\"]"));
        assert!(pgn.contains("[Result \"1-0\"]"));
        assert!(pgn.contains("1. e4 { 1:12 } e5 2. Qh5 { 0:03 }"));
        assert!(pgn.contains("Qxf7#"));
        assert_eq!(
            Chess
                .close(&state, &close_body(CloseReason::Finished, &[]))
                .unwrap()
                .summary,
            "1-0"
        );
        assert!(Chess
            .close(&start(), &close_body(CloseReason::Finished, &[]))
            .is_err());
    }

    #[test]
    fn an_illegal_move_and_a_wrong_san_are_refused() {
        let state = start();
        let err = play(
            &state,
            "alice",
            Body::Move {
                uci: "e2e5".into(),
                san: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("illegal"), "{err}");
        assert!(play(
            &state,
            "bob",
            Body::Move {
                uci: "e7e5".into(),
                san: None,
            },
        )
        .unwrap_err()
        .to_string()
        .contains("may not append"));
        assert!(!Chess.may_append(&state, 1, "move"));
        assert!(Chess.may_append(&state, 0, "move"));
        let err = play(
            &state,
            "alice",
            Body::Move {
                uci: "e2e4".into(),
                san: Some("e3".into()),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("san is e4"), "{err}");
    }

    #[test]
    fn castling_en_passant_and_promotion() {
        let state = moves(
            &start(),
            &[
                ("alice", "e2e4"),
                ("bob", "e7e5"),
                ("alice", "g1f3"),
                ("bob", "b8c6"),
                ("alice", "f1c4"),
                ("bob", "g8f6"),
                ("alice", "e1g1"),
            ],
        );
        assert_eq!(sans(&state).unwrap().last().unwrap(), "O-O");

        let ep = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("rnbqkbnr/pppp1ppp/8/4pP2/8/8/PPPPP1PP/RNBQKBNR w KQkq e6 0 3"),
                ),
                &[],
                &[],
            )
            .unwrap();
        let ep = play(
            &ep,
            "alice",
            Body::Move {
                uci: "f5e6".into(),
                san: None,
            },
        )
        .unwrap();
        assert_eq!(sans(&ep).unwrap(), ["fxe6"]);
        assert_eq!(chess_view(&ep).unwrap().captured_by_white, "P");
        assert!(!chess_view(&ep).unwrap().fen.contains("4pP"));

        let promo = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("8/P7/8/8/8/8/8/4K2k w - - 0 1"),
                ),
                &[],
                &[],
            )
            .unwrap();
        let view = chess_view(&promo).unwrap();
        assert!(view.legal_uci.iter().any(|u| u == "a7a8q"));
        let promo = play(
            &promo,
            "alice",
            Body::Move {
                uci: "a7a8q".into(),
                san: Some("a8=Q+".into()),
            },
        )
        .unwrap();
        assert_eq!(sans(&promo).unwrap(), ["a8=Q+"]);
    }

    #[test]
    fn stalemate_fifty_move_and_threefold() {
        let stale = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"),
                ),
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(stale.result.as_deref(), Some("1/2-1/2"));
        assert!(!Chess.may_append(&stale, 1, "move"));

        let fifty = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("4k3/8/8/8/8/8/4P3/4K3 w - - 99 80"),
                ),
                &[],
                &[],
            )
            .unwrap();
        assert!(fifty.result.is_none());
        let fifty = play(
            &fifty,
            "alice",
            Body::Move {
                uci: "e1d1".into(),
                san: None,
            },
        )
        .unwrap();
        assert_eq!(fifty.result.as_deref(), Some("1/2-1/2"));

        let rep = moves(
            &start(),
            &[
                ("alice", "g1f3"),
                ("bob", "g8f6"),
                ("alice", "f3g1"),
                ("bob", "f6g8"),
                ("alice", "g1f3"),
                ("bob", "g8f6"),
                ("alice", "f3g1"),
                ("bob", "f6g8"),
            ],
        );
        assert_eq!(rep.result.as_deref(), Some("1/2-1/2"));
    }

    #[test]
    fn resign_offer_and_a_move_that_declines() {
        let state = start();
        let offered = play(&state, "bob", Body::OfferDraw {}).unwrap();
        assert_eq!(offered.draw_offer, Some(1));
        assert!(
            Chess.obliged(&offered) == vec![0],
            "the side to move stays obliged"
        );
        assert!(play(&offered, "bob", Body::OfferDraw {}).is_err());
        assert!(!Chess.may_append(&offered, 1, "accept_draw"));
        let declined = play(
            &offered,
            "alice",
            Body::Move {
                uci: "e2e4".into(),
                san: None,
            },
        )
        .unwrap();
        assert!(declined.draw_offer.is_none());
        assert!(declined.result.is_none());

        let offered = play(&state, "alice", Body::OfferDraw {}).unwrap();
        let drawn = play(&offered, "bob", Body::AcceptDraw {}).unwrap();
        assert_eq!(drawn.result.as_deref(), Some("1/2-1/2"));
        assert!(Chess
            .close(&start(), &close_body(CloseReason::Agreed, &[]))
            .unwrap()
            .winners
            .is_empty());

        let resigned = play(&state, "alice", Body::Resign {}).unwrap();
        assert_eq!(resigned.result.as_deref(), Some("0-1"));
        let outcome = Chess
            .close(&start(), &close_body(CloseReason::Abandoned, &["alice"]))
            .unwrap();
        assert_eq!(outcome.summary, "0-1");
        assert_eq!(outcome.winners, vec![1]);
    }

    #[test]
    fn genesis_colour_and_quorum() {
        assert!(Chess.wants_reveals(&genesis([None, None], 2, None)));
        assert!(!Chess.wants_reveals(&genesis([Some("white"), Some("black")], 2, None)));
        assert!(Chess
            .init(&genesis([Some("white"), None], 2, None), &[], &[])
            .is_err());
        assert!(Chess
            .init(&genesis([Some("white"), Some("white")], 2, None), &[], &[])
            .is_err());
        assert!(Chess
            .init(&genesis([Some("white"), Some("black")], 1, None), &[], &[])
            .unwrap_err()
            .to_string()
            .contains("confirm_quorum"));
        let mut three = genesis([Some("white"), Some("black")], 2, None);
        three.parties.push(party("carol", None));
        three.confirm_quorum = 2;
        assert!(Chess
            .init(&three, &[], &[])
            .unwrap_err()
            .to_string()
            .contains("two parties"));

        let nonces = vec![b"alice-nonce".to_vec(), b"bob-nonce".to_vec()];
        let mut bytes = Vec::new();
        for n in &nonces {
            bytes.extend_from_slice(n);
        }
        let white = usize::from(Hash::of(&bytes).as_bytes()[0] & 1);
        let state = Chess
            .init(&genesis([None, None], 2, None), &[], &nonces)
            .unwrap();
        assert_eq!(state.white, white);
        assert_eq!(state.black, 1 - white);
        assert_eq!(Chess.obliged(&state), vec![state.white]);
    }

    #[test]
    fn the_opening_has_twenty_legal_moves_and_a_check_is_visible() {
        let state = start();
        let view = chess_view(&state).unwrap();
        assert_eq!(view.legal_uci.len(), 20);
        assert!(view.legal_uci.windows(2).all(|w| w[0] <= w[1]), "sorted");
        assert!(view.legal_uci.contains(&"e2e4".into()));
        assert!(!view.legal_uci.contains(&"e2e5".into()));
        assert_eq!(view.turn, "white");
        assert_eq!(view.turn_party, Some(0));
        assert!(!view.in_check);
        assert!(view.result.is_none());
        let pgn = pgn(&state, &[]).unwrap();
        assert!(pgn.contains("[Result \"*\"]"));
        assert!(pgn.ends_with("*\n"));
        assert_eq!(
            Chess.canonical_state(&state),
            Chess.canonical_state(&start())
        );

        let spelled = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
                ),
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(spelled.initial_fen, state.initial_fen);

        let checked = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("4k3/4Q3/8/8/8/8/8/4K3 b - - 0 1"),
                ),
                &[],
                &[],
            )
            .unwrap();
        let view = chess_view(&checked).unwrap();
        assert!(view.in_check);
        assert!(view.result.is_none());
        assert_eq!(view.turn_party, Some(1));
        assert_eq!(Chess.obliged(&checked), vec![1]);

        let bare = Chess
            .init(
                &genesis(
                    [Some("white"), Some("black")],
                    2,
                    Some("8/8/8/8/8/8/8/k6K w - - 0 1"),
                ),
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(bare.result.as_deref(), Some("1/2-1/2"));
        assert!(Chess.obliged(&bare).is_empty());
        assert!(Chess
            .init(
                &genesis([Some("white"), Some("black")], 2, Some("not a fen")),
                &[],
                &[],
            )
            .is_err());
    }

    #[test]
    fn a_resignation_ends_the_game_and_the_offerer_cannot_accept() {
        let state = start();
        assert!(play(&state, "alice", Body::AcceptDraw {}).is_err());
        assert!(!Chess.may_append(&state, 0, "nope"));
        let resigned = play(&state, "bob", Body::Resign {}).unwrap();
        assert_eq!(resigned.result.as_deref(), Some("1-0"));
        assert!(matches!(
            Chess.status(&resigned),
            Status::Finished(o) if o.summary == "1-0" && o.winners == vec![0]
        ));
        assert!(play(
            &resigned,
            "alice",
            Body::Move {
                uci: "e2e4".into(),
                san: None,
            },
        )
        .is_err());
        assert_eq!(
            Chess
                .close(&start(), &close_body(CloseReason::Abandoned, &["bob"]))
                .unwrap()
                .summary,
            "1-0"
        );
        assert!(Chess
            .close(
                &start(),
                &close_body(CloseReason::Abandoned, &["alice", "bob"])
            )
            .is_err());
    }
}
