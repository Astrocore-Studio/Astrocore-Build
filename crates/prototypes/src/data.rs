use std::collections::BTreeMap;

use serde::de::{Deserialize, Deserializer};

/// Данные компонента: общий вид для RON и YAML; движок и игровые модули сами интерпретируют поля.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Data>),
    Map(BTreeMap<String, Data>),
}

impl Data {
    /// Поле-значение по пути из карты; удобно для чтения компонентов.
    pub fn get(&self, key: &str) -> Option<&Data> {
        match self {
            Data::Map(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Data::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Data::Float(f) => Some(*f),
            Data::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Data::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Data]> {
        match self {
            Data::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Data::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Слияние родителя и потомка: карты сливаются по полям (потомок перекрывает), всё прочее
    /// заменяется потомком.
    #[must_use]
    pub fn merged_with(self, child: Data) -> Data {
        match (self, child) {
            (Data::Map(mut base), Data::Map(over)) => {
                for (k, v) in over {
                    match base.remove(&k) {
                        Some(old) => base.insert(k, old.merged_with(v)),
                        None => base.insert(k, v),
                    };
                }
                Data::Map(base)
            }
            (_, child) => child,
        }
    }

    pub(crate) fn from_ron(v: ron::Value) -> Data {
        use ron::Value as V;
        match v {
            V::Unit => Data::Unit,
            V::Bytes(b) => Data::List(b.into_iter().map(|x| Data::Int(i64::from(x))).collect()),
            V::Bool(b) => Data::Bool(b),
            V::Char(c) => Data::Str(c.to_string()),
            V::String(s) => Data::Str(s),
            V::Number(n) => match n {
                ron::value::Number::I8(i) => Data::Int(i64::from(i)),
                ron::value::Number::I16(i) => Data::Int(i64::from(i)),
                ron::value::Number::I32(i) => Data::Int(i64::from(i)),
                ron::value::Number::I64(i) => Data::Int(i),
                ron::value::Number::U8(i) => Data::Int(i64::from(i)),
                ron::value::Number::U16(i) => Data::Int(i64::from(i)),
                ron::value::Number::U32(i) => Data::Int(i64::from(i)),
                ron::value::Number::U64(i) => {
                    i64::try_from(i).map_or(Data::Float(i as f64), Data::Int)
                }
                other => Data::Float(other.into_f64()),
            },
            V::Option(o) => o.map_or(Data::Unit, |b| Data::from_ron(*b)),
            V::Seq(s) => Data::List(s.into_iter().map(Data::from_ron).collect()),
            V::Map(m) => Data::Map(
                m.into_iter()
                    .map(|(k, v)| {
                        let key = match k {
                            V::String(s) => s,
                            other => format!("{other:?}"),
                        };
                        (key, Data::from_ron(v))
                    })
                    .collect(),
            ),
        }
    }
}

impl<'de> Deserialize<'de> for Data {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Data::from_ron(ron::Value::deserialize(d)?))
    }
}
