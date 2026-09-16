//! Prefix condition programs consumed by NScriptLoadUnitUtils710047cf60.

use std::io::Cursor;

use anyhow::{bail, ensure, Result};
use binrw::{BinReaderExt, BinWriterExt};

use super::{CObjectId, CStringFixed};

#[derive(Clone, Debug)]
pub enum LoadUnitLogicToken {
    /// Names are serialized/debug data; runtime lookup uses only the GUID.
    Environment {
        id: CObjectId,
        name: CStringFixed,
    },
    /// 0 ==, 1 !=, 2 <, 3 >, 4 <=, 5 >=, 6 AND, 7 OR. Others yield false.
    Operator(u16),
    Integer(i32),
    /// Other token kinds have no payload and do not add an operand.
    Unknown(u16),
}

#[derive(Clone, Debug, Default)]
pub struct LoadUnitLogic {
    pub tokens: Vec<LoadUnitLogicToken>,
    /// Bytes beyond the declared token count; the native evaluator ignores them.
    pub trailing_data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadUnitLogicResult {
    pub value: bool,
    /// Evaluation returns when the first root expression completes. Remaining
    /// declared tokens are not evaluated, unlike an eager parse of the record.
    pub tokens_consumed: usize,
}

impl LoadUnitLogic {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let count: u16 = reader.read_le()?;
        let mut tokens = Vec::with_capacity(count as usize);
        for _ in 0..count {
            tokens.push(match reader.read_le::<u16>()? {
                0 => LoadUnitLogicToken::Environment {
                    id: reader.read_le()?,
                    name: reader.read_le()?,
                },
                1 => LoadUnitLogicToken::Operator(reader.read_le()?),
                2 => LoadUnitLogicToken::Integer(reader.read_le()?),
                kind => LoadUnitLogicToken::Unknown(kind),
            });
        }
        Ok(Self { tokens, trailing_data: data[reader.position() as usize..].to_vec() })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        ensure!(self.tokens.len() <= u16::MAX as usize, "Too many load-unit logic tokens");
        let mut writer = Cursor::new(Vec::new());
        writer.write_le(&(self.tokens.len() as u16))?;
        for token in &self.tokens {
            match token {
                LoadUnitLogicToken::Environment { id, name } => {
                    writer.write_le(&0u16)?;
                    writer.write_le(id)?;
                    writer.write_le(name)?;
                }
                LoadUnitLogicToken::Operator(op) => {
                    writer.write_le(&1u16)?;
                    writer.write_le(op)?;
                }
                LoadUnitLogicToken::Integer(value) => {
                    writer.write_le(&2u16)?;
                    writer.write_le(value)?;
                }
                LoadUnitLogicToken::Unknown(kind) => {
                    ensure!(*kind > 2, "Known token kind requires a payload");
                    writer.write_le(kind)?;
                }
            }
        }
        let mut bytes = writer.into_inner();
        bytes.extend_from_slice(&self.trailing_data);
        Ok(bytes)
    }

    /// Evaluate operators with signed i32 operands. AND/OR evaluate both operands;
    /// they do not skip variable lookups. Empty or unfinished programs return true
    /// as native does. An operand without an operator is rejected: the native
    /// implementation would access storage before its stack in that case.
    pub fn evaluate(
        &self,
        mut variable: impl FnMut(CObjectId) -> Result<i32>,
    ) -> Result<LoadUnitLogicResult> {
        let mut stack: Vec<(u16, Option<i32>)> = Vec::new();
        for (index, token) in self.tokens.iter().enumerate() {
            let mut value = match token {
                LoadUnitLogicToken::Operator(op) => {
                    stack.push((*op, None));
                    continue;
                }
                LoadUnitLogicToken::Environment { id, .. } => variable(*id)?,
                LoadUnitLogicToken::Integer(value) => *value,
                LoadUnitLogicToken::Unknown(_) => {
                    ensure!(!stack.is_empty(), "Load-unit logic token without an operator");
                    continue;
                }
            };
            loop {
                let Some((op, left)) = stack.last_mut() else {
                    bail!("Load-unit logic operand without an operator");
                };
                let Some(left_value) = *left else {
                    *left = Some(value);
                    break;
                };
                value = i32::from(match op {
                    0 => left_value == value,
                    1 => left_value != value,
                    2 => left_value < value,
                    3 => left_value > value,
                    4 => left_value <= value,
                    5 => left_value >= value,
                    6 => left_value != 0 && value != 0,
                    7 => left_value != 0 || value != 0,
                    _ => false,
                });
                stack.pop();
                if stack.is_empty() {
                    return Ok(LoadUnitLogicResult {
                        value: value != 0,
                        tokens_consumed: index + 1,
                    });
                }
            }
        }
        Ok(LoadUnitLogicResult { value: true, tokens_consumed: self.tokens.len() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_logic_uses_signed_operands_and_eager_lookups() {
        use LoadUnitLogicToken::*;
        let id = CObjectId::default();
        let variable = || Environment { id, name: CStringFixed::default() };
        let program = LoadUnitLogic {
            tokens: vec![Operator(6), Integer(0), Operator(2), Integer(-1), variable()],
            trailing_data: vec![0xaa],
        };
        let mut lookups = 0;
        let result = program
            .evaluate(|_| {
                lookups += 1;
                Ok(0)
            })
            .unwrap();
        assert_eq!(result, LoadUnitLogicResult { value: false, tokens_consumed: 5 });
        assert_eq!(lookups, 1);
        let bytes = program.to_bytes().unwrap();
        assert_eq!(LoadUnitLogic::parse(&bytes).unwrap().to_bytes().unwrap(), bytes);
        let signed = LoadUnitLogic {
            tokens: vec![Operator(2), Integer(-1), Integer(0)],
            ..Default::default()
        };
        assert!(signed.evaluate(|_| unreachable!()).unwrap().value);
    }

    #[test]
    fn prefix_logic_preserves_early_return_and_incomplete_default() {
        use LoadUnitLogicToken::*;
        let mut program = LoadUnitLogic {
            tokens: vec![Operator(0), Integer(1), Integer(2), Environment {
                id: CObjectId::default(),
                name: CStringFixed::default(),
            }],
            ..Default::default()
        };
        assert_eq!(program.evaluate(|_| unreachable!()).unwrap(), LoadUnitLogicResult {
            value: false,
            tokens_consumed: 3
        });
        program.tokens.truncate(2);
        assert!(program.evaluate(|_| unreachable!()).unwrap().value);
        program.tokens = vec![Integer(1)];
        assert!(program.evaluate(|_| unreachable!()).is_err());
    }
}
