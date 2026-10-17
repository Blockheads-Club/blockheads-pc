//! Java types, descriptors and values.

/// A reference to a heap object of the fake JVM. `Obj::NULL` (0) is never a live object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Obj(pub u32);

impl Obj {
    pub const NULL: Obj = Obj(0);
}

/// A class of the fake JVM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassId(pub u32);

/// The JNI primitive/reference type of a method parameter, return value or field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ty {
    Void,
    Bool,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Ref,
}

impl Ty {
    /// The type for a descriptor's first character (`L` and `[` are references).
    pub fn from_desc(c: char) -> Option<Ty> {
        Some(match c {
            'V' => Ty::Void,
            'Z' => Ty::Bool,
            'B' => Ty::Byte,
            'C' => Ty::Char,
            'S' => Ty::Short,
            'I' => Ty::Int,
            'J' => Ty::Long,
            'F' => Ty::Float,
            'D' => Ty::Double,
            'L' | '[' => Ty::Ref,
            _ => return None,
        })
    }

    /// Size of one array element in bytes (references are 4 bytes in the guest).
    pub fn size(self) -> usize {
        match self {
            Ty::Void => 0,
            Ty::Bool | Ty::Byte => 1,
            Ty::Char | Ty::Short => 2,
            Ty::Int | Ty::Float | Ty::Ref => 4,
            Ty::Long | Ty::Double => 8,
        }
    }

    /// Descriptor character of a primitive (`L` for references).
    pub fn desc(self) -> char {
        match self {
            Ty::Void => 'V',
            Ty::Bool => 'Z',
            Ty::Byte => 'B',
            Ty::Char => 'C',
            Ty::Short => 'S',
            Ty::Int => 'I',
            Ty::Long => 'J',
            Ty::Float => 'F',
            Ty::Double => 'D',
            Ty::Ref => 'L',
        }
    }

    /// Zero/null value of this type.
    pub fn default_value(self) -> Value {
        match self {
            Ty::Void => Value::Void,
            Ty::Long => Value::Long(0),
            Ty::Float => Value::Float(0.0),
            Ty::Double => Value::Double(0.0),
            Ty::Ref => Value::Ref(None),
            _ => Value::Int(0),
        }
    }
}

/// A Java value. `boolean`, `byte`, `char` and `short` travel as `Int`, as in JNI varargs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Void,
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Ref(Option<Obj>),
}

impl Value {
    pub fn bool(b: bool) -> Value {
        Value::Int(b as i32)
    }

    /// Integer view (`boolean/byte/char/short/int`), 0 for anything else.
    pub fn as_int(&self) -> i32 {
        match *self {
            Value::Int(v) => v,
            Value::Long(v) => v as i32,
            _ => 0,
        }
    }

    pub fn as_long(&self) -> i64 {
        match *self {
            Value::Long(v) => v,
            Value::Int(v) => v as i64,
            _ => 0,
        }
    }

    pub fn as_float(&self) -> f32 {
        match *self {
            Value::Float(v) => v,
            Value::Double(v) => v as f32,
            _ => 0.0,
        }
    }

    pub fn as_double(&self) -> f64 {
        match *self {
            Value::Double(v) => v,
            Value::Float(v) => v as f64,
            _ => 0.0,
        }
    }

    pub fn as_ref(&self) -> Option<Obj> {
        match *self {
            Value::Ref(o) => o,
            _ => None,
        }
    }
}

/// A parsed method descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sig {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

/// Parse a method descriptor such as `(ILjava/lang/String;[J)V`.
pub fn parse_sig(s: &str) -> Option<Sig> {
    let rest = s.strip_prefix('(')?;
    let (args, ret) = rest.split_once(')')?;
    let mut params = Vec::new();
    let mut it = args.chars().peekable();
    while let Some(c) = it.next() {
        let mut c = c;
        let mut array = false;
        while c == '[' {
            array = true;
            c = it.next()?;
        }
        let ty = Ty::from_desc(c)?;
        if c == 'L' {
            loop {
                if it.next()? == ';' {
                    break;
                }
            }
        }
        if ty == Ty::Void {
            return None;
        }
        params.push(if array { Ty::Ref } else { ty });
    }
    let mut r = ret.chars();
    let first = r.next()?;
    let ret = if first == '[' { Ty::Ref } else { Ty::from_desc(first)? };
    Some(Sig { params, ret })
}

/// The type of a field descriptor (`I`, `Ljava/lang/String;`, `[B`, ...).
pub fn parse_field_ty(s: &str) -> Option<Ty> {
    s.chars().next().and_then(Ty::from_desc).filter(|t| *t != Ty::Void)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors() {
        let s = parse_sig("(ILjava/lang/String;[JZ[[Ljava/lang/Object;D)[B").unwrap();
        assert_eq!(s.params, [Ty::Int, Ty::Ref, Ty::Ref, Ty::Bool, Ty::Ref, Ty::Double]);
        assert_eq!(s.ret, Ty::Ref);
        assert_eq!(parse_sig("()V").unwrap(), Sig { params: vec![], ret: Ty::Void });
        for bad in ["", "I", "(I", "(Q)V", "(V)V", "(Ljava/lang/String)V", "(I)", "(I)Q", "("] {
            assert!(parse_sig(bad).is_none(), "{bad}");
        }
        assert_eq!(parse_field_ty("[I"), Some(Ty::Ref));
        assert_eq!(parse_field_ty("J"), Some(Ty::Long));
        assert_eq!(parse_field_ty("V"), None);
        assert_eq!(parse_field_ty(""), None);
    }

    #[test]
    fn types_and_values() {
        for (c, size) in
            [('Z', 1), ('B', 1), ('C', 2), ('S', 2), ('I', 4), ('F', 4), ('J', 8), ('D', 8), ('L', 4), ('V', 0)]
        {
            let t = Ty::from_desc(c).unwrap();
            assert_eq!(t.size(), size);
            assert_eq!(Ty::from_desc(t.desc()), Some(t));
        }
        assert_eq!(Ty::from_desc('['), Some(Ty::Ref));
        assert_eq!(Ty::from_desc('x'), None);
        assert_eq!(Ty::Void.default_value(), Value::Void);
        assert_eq!(Ty::Long.default_value(), Value::Long(0));
        assert_eq!(Ty::Float.default_value(), Value::Float(0.0));
        assert_eq!(Ty::Double.default_value(), Value::Double(0.0));
        assert_eq!(Ty::Ref.default_value(), Value::Ref(None));
        assert_eq!(Ty::Short.default_value(), Value::Int(0));
        assert_eq!(Value::bool(true), Value::Int(1));
        assert_eq!(Value::Int(7).as_int(), 7);
        assert_eq!(Value::Long(1 << 33).as_int(), 0);
        assert_eq!(Value::Float(1.0).as_int(), 0);
        assert_eq!(Value::Int(-3).as_long(), -3);
        assert_eq!(Value::Long(5).as_long(), 5);
        assert_eq!(Value::Void.as_long(), 0);
        assert_eq!(Value::Float(1.5).as_float(), 1.5);
        assert_eq!(Value::Double(2.5).as_float(), 2.5);
        assert_eq!(Value::Int(1).as_float(), 0.0);
        assert_eq!(Value::Double(2.5).as_double(), 2.5);
        assert_eq!(Value::Float(1.5).as_double(), 1.5);
        assert_eq!(Value::Int(1).as_double(), 0.0);
        assert_eq!(Value::Ref(Some(Obj(3))).as_ref(), Some(Obj(3)));
        assert_eq!(Value::Int(3).as_ref(), None);
    }
}
