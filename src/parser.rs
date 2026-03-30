use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct CellUpdate {
    pub offset: i64,
    pub factor: i32,
}

#[derive(Clone, Debug)]
pub enum Op {
    PtrAdd(i64),
    CellAdd(i32),
    ClearCell,
    AddScaled(Vec<CellUpdate>),
    Output,
    Input,
    LoopStart,
    LoopEnd,
}

fn push_op(ops: &mut Vec<Op>, op: Op) {
    match op {
        Op::PtrAdd(0) | Op::CellAdd(0) => {}
        Op::PtrAdd(delta) => {
            if let Some(Op::PtrAdd(prev)) = ops.last_mut() {
                *prev += delta;
                if *prev == 0 {
                    ops.pop();
                }
            } else {
                ops.push(Op::PtrAdd(delta));
            }
        }
        Op::CellAdd(delta) => {
            if let Some(Op::CellAdd(prev)) = ops.last_mut() {
                *prev += delta;
                if *prev == 0 {
                    ops.pop();
                }
            } else {
                ops.push(Op::CellAdd(delta));
            }
        }
        other => ops.push(other),
    }
}

fn compute_loop_pairs(ops: &[Op]) -> anyhow::Result<Vec<usize>> {
    let mut loop_pairs = vec![usize::MAX; ops.len()];
    let mut stack = Vec::new();

    for (index, op) in ops.iter().enumerate() {
        match op {
            Op::LoopStart => stack.push(index),
            Op::LoopEnd => {
                let start = stack
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("internal unmatched closing bracket"))?;
                loop_pairs[start] = index;
                loop_pairs[index] = start;
            }
            _ => {}
        }
    }

    if !stack.is_empty() {
        anyhow::bail!("internal unmatched opening bracket");
    }

    Ok(loop_pairs)
}

fn try_optimize_clear_loop(body: &[Op]) -> Option<Op> {
    match body {
        [Op::CellAdd(delta)] if delta.rem_euclid(2) != 0 => Some(Op::ClearCell),
        _ => None,
    }
}

fn try_optimize_add_scaled_loop(body: &[Op]) -> Option<Op> {
    let mut pointer_offset = 0i64;
    let mut current_delta = 0i32;
    let mut updates = BTreeMap::new();

    for op in body {
        match op {
            Op::PtrAdd(delta) => pointer_offset += delta,
            Op::CellAdd(delta) => {
                if pointer_offset == 0 {
                    current_delta += delta;
                } else {
                    *updates.entry(pointer_offset).or_insert(0) += delta;
                }
            }
            _ => return None,
        }
    }

    if pointer_offset != 0 || current_delta.rem_euclid(256) != 255 {
        return None;
    }

    let updates: Vec<_> = updates
        .into_iter()
        .filter_map(|(offset, factor)| {
            let wrapped = factor.rem_euclid(256);
            (wrapped != 0).then_some(CellUpdate { offset, factor })
        })
        .collect();

    if updates.is_empty() {
        Some(Op::ClearCell)
    } else {
        Some(Op::AddScaled(updates))
    }
}

fn try_optimize_loop(body: &[Op]) -> Option<Op> {
    try_optimize_clear_loop(body).or_else(|| try_optimize_add_scaled_loop(body))
}

fn optimize_range(
    ops: &[Op],
    loop_pairs: &[usize],
    start: usize,
    end: usize,
) -> anyhow::Result<Vec<Op>> {
    let mut optimized = Vec::new();
    let mut index = start;

    while index < end {
        match &ops[index] {
            Op::LoopStart => {
                let loop_end = loop_pairs[index];
                let body = optimize_range(ops, loop_pairs, index + 1, loop_end)?;
                if let Some(op) = try_optimize_loop(&body) {
                    push_op(&mut optimized, op);
                } else {
                    optimized.push(Op::LoopStart);
                    optimized.extend(body);
                    optimized.push(Op::LoopEnd);
                }
                index = loop_end + 1;
            }
            Op::LoopEnd => anyhow::bail!("internal unexpected loop terminator"),
            other => {
                push_op(&mut optimized, other.clone());
                index += 1;
            }
        }
    }

    Ok(optimized)
}

fn optimize_ops(ops: &[Op]) -> anyhow::Result<Vec<Op>> {
    let loop_pairs = compute_loop_pairs(ops)?;
    optimize_range(ops, &loop_pairs, 0, ops.len())
}

pub fn parse_brainfuck<T>(chars: T) -> anyhow::Result<Vec<Op>>
where
    T: Iterator<Item = u8>,
{
    let mut ops = Vec::with_capacity(chars.size_hint().0 / 2 + 50); // just some heuristic value here
    let mut loop_depth = 0usize;
    let mut chars = chars.peekable();

    while let Some(ch) = chars.next() {
        match ch {
            b'>' | b'<' => {
                let mut delta: i64 = if ch == b'>' { 1 } else { -1 };
                while let Some(next) = chars.peek() {
                    if *next == b'>' {
                        delta += 1;
                        chars.next();
                    } else if *next == b'<' {
                        delta -= 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                push_op(&mut ops, Op::PtrAdd(delta));
            }
            b'+' | b'-' => {
                let mut delta: i32 = if ch == b'+' { 1 } else { -1 };
                while let Some(next) = chars.peek() {
                    if *next == b'+' {
                        delta += 1;
                        chars.next();
                    } else if *next == b'-' {
                        delta -= 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                push_op(&mut ops, Op::CellAdd(delta));
            }
            b'.' => ops.push(Op::Output),
            b',' => ops.push(Op::Input),
            b'[' => {
                loop_depth += 1;
                ops.push(Op::LoopStart);
            }
            b']' => {
                if loop_depth == 0 {
                    anyhow::bail!("unmatched closing bracket ']' found");
                }
                loop_depth -= 1;
                ops.push(Op::LoopEnd);
            }
            _ => {}
        }
    }

    if loop_depth != 0 {
        anyhow::bail!("unmatched opening bracket '[' found");
    }

    optimize_ops(&ops)
}
