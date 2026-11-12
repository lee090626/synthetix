//! Minimal direct WebAssembly emitter for AMI `i32` expressions.

use axiom_ami::Expr;
use wasmi::{Engine, Linker, Module, Store, Val};

/// Returns the number of expression instructions emitted for an AMI tree.
pub fn instruction_cost(expr: &Expr) -> usize {
    match expr {
        Expr::Arg(_) | Expr::Const(_) => 1,
        Expr::Add(lhs, rhs)
        | Expr::Sub(lhs, rhs)
        | Expr::Mul(lhs, rhs)
        | Expr::Div(lhs, rhs)
        | Expr::Shl(lhs, rhs)
        | Expr::BitAnd(lhs, rhs)
        | Expr::BitOr(lhs, rhs)
        | Expr::BitXor(lhs, rhs)
        | Expr::Eq(lhs, rhs)
        | Expr::Ne(lhs, rhs)
        | Expr::Lt(lhs, rhs)
        | Expr::Le(lhs, rhs)
        | Expr::Gt(lhs, rhs)
        | Expr::Ge(lhs, rhs)
        | Expr::SLt(lhs, rhs)
        | Expr::SLe(lhs, rhs)
        | Expr::SGt(lhs, rhs)
        | Expr::SGe(lhs, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs) => 1 + instruction_cost(lhs) + instruction_cost(rhs),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => {
            1 + instruction_cost(condition)
                + instruction_cost(then_branch)
                + instruction_cost(else_branch)
        }
        Expr::ArrayGet { elements, index } => {
            instruction_cost(index)
                + elements.iter().map(instruction_cost).sum::<usize>()
                + elements.len() * 4
        }
        Expr::Guard { condition, value } => {
            1 + instruction_cost(condition) + instruction_cost(value)
        }
    }
}

/// Emits a WebAssembly module exporting the expression as `run`.
pub fn emit(expr: &Expr, param_count: u32) -> Result<Vec<u8>, String> {
    emit_functions(&[("run", expr, param_count)])
}

/// Emits one module containing several named pure functions.
pub fn emit_functions(functions: &[(&str, &Expr, u32)]) -> Result<Vec<u8>, String> {
    if functions.is_empty() {
        return Err("cannot emit an empty WebAssembly module".to_string());
    }
    let mut names = std::collections::HashSet::new();
    for (name, _, _) in functions {
        if name.is_empty() || !names.insert(*name) {
            return Err(format!("duplicate or empty WASM export: {name}"));
        }
    }

    let mut module = b"\0asm\x01\0\0\0".to_vec();

    let mut types = Vec::new();
    push_uleb(functions.len() as u64, &mut types);
    for (_, _, param_count) in functions {
        types.push(0x60);
        push_uleb(*param_count as u64, &mut types);
        types.extend(std::iter::repeat_n(0x7f, *param_count as usize));
        types.extend([1, 0x7f]);
    }
    push_section(1, &types, &mut module);

    let mut function_types = Vec::new();
    push_uleb(functions.len() as u64, &mut function_types);
    for index in 0..functions.len() {
        push_uleb(index as u64, &mut function_types);
    }
    push_section(3, &function_types, &mut module);

    let mut exports = Vec::new();
    push_uleb(functions.len() as u64, &mut exports);
    for (index, (name, _, _)) in functions.iter().enumerate() {
        push_name(name, &mut exports);
        exports.extend([0, index as u8]);
    }
    push_section(7, &exports, &mut module);

    let mut code = Vec::new();
    push_uleb(functions.len() as u64, &mut code);
    for (_, expression, param_count) in functions {
        let mut body = vec![0];
        encode_expr(expression, *param_count, &mut body)?;
        body.push(0x0b);
        push_uleb(body.len() as u64, &mut code);
        code.extend(body);
    }
    push_section(10, &code, &mut module);

    Ok(module)
}

/// Executes a generated one-argument `u32 -> u32` module.
pub fn run(wasm: &[u8], input: u32) -> Result<u32, String> {
    run_with_args(wasm, &[input])
}

/// Executes a generated module with `u32` arguments and one `u32` result.
pub fn run_with_args(wasm: &[u8], inputs: &[u32]) -> Result<u32, String> {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).map_err(|error| format!("invalid WASM: {error}"))?;
    let mut store = Store::new(&engine, ());
    let linker = Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| format!("cannot instantiate WASM: {error}"))?;
    let function = instance
        .get_func(&store, "run")
        .ok_or_else(|| "invalid run export: missing function".to_string())?;
    let inputs = inputs
        .iter()
        .map(|input| Val::I32(*input as i32))
        .collect::<Vec<_>>();
    let mut outputs = vec![Val::I32(0)];
    function
        .call(&mut store, &inputs, &mut outputs)
        .map_err(|error| format!("WASM execution failed: {error}"))?;
    outputs
        .first()
        .and_then(Val::i32)
        .map(|result| result as u32)
        .ok_or_else(|| "WASM run returned a non-i32 result".to_string())
}

fn encode_expr(expr: &Expr, param_count: u32, output: &mut Vec<u8>) -> Result<(), String> {
    match expr {
        Expr::Arg(index) => {
            if u32::from(*index) >= param_count {
                return Err(format!("expression references missing argument {index}"));
            }
            output.extend([0x20, *index]); // local.get
        }
        Expr::Const(value) => {
            output.push(0x41); // i32.const
            push_sleb((*value as i32) as i64, output);
        }
        Expr::Add(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x6a); // i32.add
        }
        Expr::Sub(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x6b); // i32.sub
        }
        Expr::Mul(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x6c); // i32.mul
        }
        Expr::Div(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x6e); // i32.div_u
        }
        Expr::Shl(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x74); // i32.shl
        }
        Expr::BitAnd(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x71); // i32.and
        }
        Expr::BitOr(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x72); // i32.or
        }
        Expr::BitXor(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x73); // i32.xor
        }
        Expr::Eq(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x46); // i32.eq
        }
        Expr::Ne(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x47); // i32.ne
        }
        Expr::Lt(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x49); // i32.lt_u
        }
        Expr::Le(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x4d); // i32.le_u
        }
        Expr::Gt(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x4b); // i32.gt_u
        }
        Expr::Ge(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x4f); // i32.ge_u
        }
        Expr::SLt(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x48); // i32.lt_s
        }
        Expr::SLe(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x4c); // i32.le_s
        }
        Expr::SGt(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x4a); // i32.gt_s
        }
        Expr::SGe(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            encode_expr(rhs, param_count, output)?;
            output.push(0x4e); // i32.ge_s
        }
        Expr::And(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            output.extend([0x04, 0x7f]); // if (result i32)
            encode_expr(rhs, param_count, output)?;
            output.push(0x45); // i32.eqz
            output.push(0x45); // normalize to 0 or 1
            output.push(0x05); // else
            output.extend([0x41, 0x00]);
            output.push(0x0b);
        }
        Expr::Or(lhs, rhs) => {
            encode_expr(lhs, param_count, output)?;
            output.extend([0x04, 0x7f]); // if (result i32)
            output.extend([0x41, 0x01]);
            output.push(0x05); // else
            encode_expr(rhs, param_count, output)?;
            output.push(0x45); // i32.eqz
            output.push(0x45); // normalize to 0 or 1
            output.push(0x0b);
        }
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => {
            encode_expr(condition, param_count, output)?;
            output.extend([0x04, 0x7f]); // if (result i32)
            encode_expr(then_branch, param_count, output)?;
            output.push(0x05); // else
            encode_expr(else_branch, param_count, output)?;
            output.push(0x0b); // end
        }
        Expr::ArrayGet { elements, index } => {
            if elements.is_empty() {
                return Err("cannot read from an empty fixed array".to_string());
            }
            for (array_index, element) in elements.iter().enumerate() {
                encode_expr(index, param_count, output)?;
                output.push(0x41);
                push_sleb(array_index as i64, output);
                output.push(0x46); // i32.eq
                output.extend([0x04, 0x7f]); // if (result i32)
                encode_expr(element, param_count, output)?;
                output.push(0x05); // else
            }
            output.push(0x00); // unreachable on an invalid index
            for _ in elements {
                output.push(0x0b); // end
            }
        }
        Expr::Guard { condition, value } => {
            encode_expr(condition, param_count, output)?;
            output.extend([0x04, 0x7f]); // if (result i32)
            encode_expr(value, param_count, output)?;
            output.push(0x05); // else
            output.push(0x00); // unreachable when the condition is false
            output.push(0x0b); // end
        }
    }
    Ok(())
}

fn push_section(id: u8, payload: &[u8], output: &mut Vec<u8>) {
    output.push(id);
    push_uleb(payload.len() as u64, output);
    output.extend(payload);
}

fn push_name(name: &str, output: &mut Vec<u8>) {
    push_uleb(name.len() as u64, output);
    output.extend(name.as_bytes());
}

fn push_uleb(mut value: u64, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn push_sleb(mut value: i64, output: &mut Vec<u8>) {
    loop {
        let byte = (value as u8) & 0x7f;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        output.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_mul8_wasm_module() {
        let expr = Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)));
        let module = emit(&expr, 1).unwrap();

        assert_eq!(&module[..8], b"\0asm\x01\0\0\0");
        assert!(module.windows(3).any(|window| window == b"run"));
    }

    #[test]
    fn runs_emitted_mul8_module() {
        let expr = Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)));
        let module = emit(&expr, 1).unwrap();

        assert_eq!(run(&module, 5).unwrap(), 40);
        assert_eq!(run(&module, u32::MAX).unwrap(), u32::MAX.wrapping_mul(8));
    }

    #[test]
    fn emits_and_runs_two_argument_module() {
        let expr = Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        let module = emit(&expr, 2).unwrap();

        assert_eq!(run_with_args(&module, &[5, 7]).unwrap(), 12);
    }

    #[test]
    fn emits_and_runs_zero_argument_module() {
        let module = emit(&Expr::Const(42), 0).unwrap();

        assert_eq!(run_with_args(&module, &[]).unwrap(), 42);
    }

    #[test]
    fn counts_emitted_expression_instructions() {
        let expr = Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)));

        assert_eq!(instruction_cost(&expr), 3);
    }

    #[test]
    fn emits_and_runs_conditional_module() {
        let expr = Expr::Select {
            condition: Box::new(Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            then_branch: Box::new(Expr::Arg(0)),
            else_branch: Box::new(Expr::Arg(1)),
        };
        let module = emit(&expr, 2).unwrap();

        assert_eq!(run_with_args(&module, &[2, 5]).unwrap(), 2);
        assert_eq!(run_with_args(&module, &[8, 5]).unwrap(), 5);
    }

    #[test]
    fn emits_and_runs_signed_comparison() {
        let expr = Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        let module = emit(&expr, 2).unwrap();

        assert_eq!(run_with_args(&module, &[u32::MAX, 1]).unwrap(), 1);
        assert_eq!(run_with_args(&module, &[1, u32::MAX]).unwrap(), 0);
    }

    #[test]
    fn emits_and_runs_scalar_bitwise_ops() {
        let expr = Expr::BitXor(
            Box::new(Expr::BitAnd(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            Box::new(Expr::Sub(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1)))),
        );
        let module = emit(&expr, 2).unwrap();

        assert_eq!(run_with_args(&module, &[0b1100, 0b1010]).unwrap(), 0b0011);
    }

    #[test]
    fn emits_several_named_exports() {
        let add = Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        let mul = Expr::Mul(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        let module = emit_functions(&[("add", &add, 2), ("mul", &mul, 2)]).unwrap();

        let engine = Engine::default();
        let module = Module::new(&engine, &module).unwrap();
        let mut store = Store::new(&engine, ());
        let instance = Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .unwrap();
        let add = instance.get_func(&store, "add").unwrap();
        let mul = instance.get_func(&store, "mul").unwrap();
        let inputs = [Val::I32(6), Val::I32(7)];
        let mut output = [Val::I32(0)];
        add.call(&mut store, &inputs, &mut output).unwrap();
        assert_eq!(output[0].i32(), Some(13));
        mul.call(&mut store, &inputs, &mut output).unwrap();
        assert_eq!(output[0].i32(), Some(42));
    }
}
