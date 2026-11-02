use knr_core::ast::*;
use knr_core::binary::const_pool::{ConstantPool, TypePool};
use knr_core::{decode_knrbin, encode_knrbin, format_ssa, parse_ssa};
use proptest::prelude::*;

fn arb_type() -> impl Strategy<Value = Type> {
    let leaf = prop_oneof![
        Just(Type::I64),
        Just(Type::F64),
        Just(Type::Bool),
        Just(Type::Str),
        Just(Type::Buf),
        Just(Type::Void),
    ];
    leaf.prop_recursive(3, 16, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|t| Type::Vec(Box::new(t))),
            (inner.clone(), inner.clone()).prop_map(|(k, v)| Type::Map(Box::new(k), Box::new(v))),
            inner.prop_map(|t| Type::Chan(Box::new(t))),
        ]
    })
}

fn arb_effect() -> impl Strategy<Value = Effect> {
    prop_oneof![
        Just(Effect::Net),
        Just(Effect::Ui),
        Just(Effect::Hw),
        Just(Effect::Async),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_type_pool_roundtrip(types in prop::collection::vec(arb_type(), 1..10)) {
        let mut pool = TypePool::new();
        let mut ids = Vec::new();
        for ty in &types {
            let id = pool.insert(ty).expect("Failed to insert type");
            ids.push((id, ty.clone()));
        }

        let mut bytes = Vec::new();
        pool.encode(&mut bytes).expect("Failed to encode TypePool");

        let mut cursor = &bytes[..];
        let decoded = TypePool::decode(&mut cursor).expect("Failed to decode TypePool");

        for (id, original_ty) in ids {
            let recovered = decoded.get(id).expect("Failed to get type from decoded pool");
            prop_assert_eq!(recovered, original_ty);
        }
    }

    #[test]
    fn prop_const_pool_roundtrip(
        strings in prop::collection::vec("[a-zA-Z0-9_]{1,30}", 1..20),
        floats in prop::collection::vec(any::<f64>(), 1..20),
        ints in prop::collection::vec(any::<i64>(), 1..20),
    ) {
        let mut pool = ConstantPool::new();
        for s in &strings {
            pool.insert_str(s).expect("Insert str failed");
        }
        for f in &floats {
            pool.insert_float(*f).expect("Insert float failed");
        }
        for i in &ints {
            pool.insert_int(*i).expect("Insert int failed");
        }

        let mut bytes = Vec::new();
        pool.encode(&mut bytes).expect("Encode const pool failed");

        let mut cursor = &bytes[..];
        let mut decoded = ConstantPool::decode(&mut cursor).expect("Decode const pool failed");

        for s in &strings {
            let idx = decoded.insert_str(s).expect("Lookup str failed");
            prop_assert_eq!(&decoded.strings[idx as usize], s);
        }
        for f in &floats {
            let idx = decoded.insert_float(*f).expect("Lookup float failed");
            prop_assert_eq!(decoded.floats[idx as usize].to_bits(), f.to_bits());
        }
        for i in &ints {
            let idx = decoded.insert_int(*i).expect("Lookup int failed");
            prop_assert_eq!(decoded.ints[idx as usize], *i);
        }
    }

    #[test]
    fn prop_arbitrary_linear_types_predicate(ty in arb_type()) {
        let is_scalar = matches!(ty, Type::I64 | Type::F64 | Type::Bool | Type::Void);
        prop_assert_eq!(ty.is_linear(), !is_scalar);
    }

    #[test]
    fn prop_generated_module_bijection(
        val_i64 in any::<i64>(),
        val_f64 in any::<f64>().prop_filter("finite float", |f| f.is_finite()),
        val_bool in any::<bool>(),
        str_val in "[a-zA-Z0-9 ]{1,20}",
        effects in prop::collection::vec(arb_effect(), 0..4),
    ) {
        // Construct canonical SSA program
        let mut effects_dedup = Vec::new();
        for eff in effects {
            if !effects_dedup.contains(&eff) {
                effects_dedup.push(eff);
            }
        }
        effects_dedup.sort_by_key(|e| match e {
            Effect::Net => 0,
            Effect::Ui => 1,
            Effect::Hw => 2,
            Effect::Async => 3,
        });
        let eff_str = if effects_dedup.is_empty() {
            String::new()
        } else {
            format!(" effects({})", effects_dedup.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", "))
        };

        let f64_str = {
            let s = val_f64.to_string();
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                format!("{s}.0")
            } else {
                s
            }
        };

        let ssa = format!(
"fn generated_func(%0: i64) -> i64{eff_str} {{
bb0:
    %1 = const_i64 {val_i64}
    %2 = const_f64 {f64_str}
    %3 = const_bool {val_bool}
    %4 = const_str \"{str_val}\"
    %5 = add %0 %1
    free %4
    ret %5
}}
");

        let parsed = parse_ssa(&ssa).expect("Parse SSA failed");
        let encoded = encode_knrbin(&parsed).expect("Encode knrbin failed");
        let decoded = decode_knrbin(&encoded).expect("Decode knrbin failed");
        let formatted = format_ssa(&decoded);

        prop_assert_eq!(formatted, ssa);
    }
}
