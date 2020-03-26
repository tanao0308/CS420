use core::fmt;
use core::mem;
use failure::Fail;
use std::collections::HashMap;

use itertools::izip;

use crate::ir::*;
use crate::*;

// TODO: the variants of Value will be added in the future
#[derive(Debug, PartialEq, Clone)]
pub enum Value {
    Unit,
    Int {
        value: u128,
        width: usize,
        is_signed: bool,
    },
    Float {
        /// `value` may be `f32`, but it is possible to consider it as `f64`.
        ///
        /// * Casting from an f32 to an f64 is perfect and lossless (f32 -> f64)
        /// * Casting from an f64 to an f32 will produce the closest possible value (f64 -> f32)
        /// https://doc.rust-lang.org/stable/reference/expressions/operator-expr.html#type-cast-expressions
        value: f64,
        width: usize,
    },
    Pointer {
        bid: Option<usize>,
        offset: usize,
    },
}

impl Value {
    #[inline]
    fn unit() -> Self {
        Self::Unit
    }

    #[inline]
    pub fn int(value: u128, width: usize, is_signed: bool) -> Self {
        Self::Int {
            value,
            width,
            is_signed,
        }
    }

    #[inline]
    fn float(value: f64, width: usize) -> Self {
        Self::Float { value, width }
    }

    #[inline]
    fn pointer(bid: Option<usize>, offset: usize) -> Self {
        Self::Pointer { bid, offset }
    }

    #[inline]
    fn get_int(self) -> Option<(u128, usize, bool)> {
        if let Value::Int {
            value,
            width,
            is_signed,
        } = self
        {
            Some((value, width, is_signed))
        } else {
            None
        }
    }

    #[inline]
    fn get_pointer(self) -> Option<(Option<usize>, usize)> {
        if let Value::Pointer { bid, offset } = self {
            Some((bid, offset))
        } else {
            None
        }
    }

    #[inline]
    fn nullptr() -> Self {
        Self::Pointer {
            bid: None,
            offset: 0,
        }
    }

    #[inline]
    fn default_from_dtype(dtype: &Dtype) -> Self {
        match dtype {
            ir::Dtype::Unit { .. } => Self::unit(),
            ir::Dtype::Int {
                width, is_signed, ..
            } => Self::int(u128::default(), *width, *is_signed),
            ir::Dtype::Float { width, .. } => Self::float(f64::default(), *width),
            ir::Dtype::Pointer { .. } => Self::nullptr(),
            ir::Dtype::Function { .. } => panic!("function types do not have a default value"),
        }
    }
}

#[derive(Debug, PartialEq, Fail)]
pub enum InterpreterError {
    #[fail(display = "current block is unreachable")]
    Unreachable,
    #[fail(display = "ir has no main function")]
    NoMainFunction,
    #[fail(display = "ir has no function definition of {} function", func_name)]
    NoFunctionDefinition { func_name: String },
    #[fail(display = "{}:{} / {}", func_name, pc, msg)]
    Misc {
        func_name: String,
        pc: Pc,
        msg: String,
    },
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Pc {
    pub bid: BlockId,
    pub iid: usize,
}

impl fmt::Display for Pc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.bid, self.iid)
    }
}

impl Pc {
    fn new(bid: BlockId) -> Pc {
        Pc { bid, iid: 0 }
    }

    fn increment(&mut self) {
        self.iid += 1;
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
struct RegisterMap {
    inner: HashMap<RegisterId, Value>,
}

impl RegisterMap {
    fn read(&self, rid: RegisterId) -> &Value {
        self.inner
            .get(&rid)
            .expect("`rid` must be assigned before it can be used")
    }

    fn write(&mut self, rid: RegisterId, value: Value) {
        let _ = self.inner.insert(rid, value);
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
/// Bidirectional map between the name of a global variable and memory box id
struct GlobalMap {
    /// Map name of a global variable to memory box id
    ///
    /// Since IR treats global variable as `Constant::GlobalVariable`,
    /// the interpreter should be able to generate pointer values by infer 'bid'
    /// from the 'name' of the global variable.
    var_to_bid: HashMap<String, usize>,
    /// Map memory box id to the name of a global variable
    ///
    /// When a function call occurs, the interpreter should be able to find `name` of the function
    /// from `bid` of the `callee` which is a function pointer.
    bid_to_var: HashMap<usize, String>,
}

impl GlobalMap {
    /// Create a bi-directional mapping between `var` and `bid`.
    fn insert(&mut self, var: String, bid: usize) -> Result<(), InterpreterError> {
        if self.var_to_bid.insert(var.clone(), bid).is_some() {
            panic!("variable name should be unique in IR")
        }
        if self.bid_to_var.insert(bid, var).is_some() {
            panic!("`bid` is connected to only one `var`")
        }

        Ok(())
    }

    fn get_bid(&self, var: &str) -> Option<usize> {
        self.var_to_bid.get(var).cloned()
    }

    fn get_var(&self, bid: usize) -> Option<String> {
        self.bid_to_var.get(&bid).cloned()
    }
}

#[derive(Debug, PartialEq, Clone)]
struct StackFrame<'i> {
    pub pc: Pc,
    pub registers: RegisterMap,
    pub func_name: String,
    pub func_def: &'i FunctionDefinition,
}

impl<'i> StackFrame<'i> {
    fn new(bid: BlockId, func_name: String, func_def: &'i FunctionDefinition) -> Self {
        StackFrame {
            pc: Pc::new(bid),
            registers: Default::default(),
            func_name,
            func_def,
        }
    }
}

mod calculator {
    use super::Value;
    use lang_c::ast;

    // TODO: change to template function in the future
    pub fn calculate_binary_operator_expression(
        op: &ast::BinaryOperator,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, ()> {
        match (op, lhs, rhs) {
            (
                op,
                Value::Int {
                    value: lhs,
                    width: lhs_w,
                    is_signed: lhs_s,
                },
                Value::Int {
                    value: rhs,
                    width: rhs_w,
                    is_signed: rhs_s,
                },
            ) => {
                assert_eq!(lhs_w, rhs_w);
                assert_eq!(lhs_s, rhs_s);

                match op {
                    ast::BinaryOperator::Plus => Ok(Value::int(lhs + rhs, lhs_w, lhs_s)),
                    ast::BinaryOperator::Minus => Ok(Value::int(lhs - rhs, lhs_w, lhs_s)),
                    ast::BinaryOperator::Multiply => Ok(Value::int(lhs * rhs, lhs_w, lhs_s)),
                    ast::BinaryOperator::Equals => {
                        let result = if lhs == rhs { 1 } else { 0 };
                        Ok(Value::int(result, 1, lhs_s))
                    }
                    ast::BinaryOperator::NotEquals => {
                        let result = if lhs != rhs { 1 } else { 0 };
                        Ok(Value::int(result, 1, lhs_s))
                    }
                    ast::BinaryOperator::Less => {
                        let result = if lhs < rhs { 1 } else { 0 };
                        Ok(Value::int(result, 1, lhs_s))
                    }
                    ast::BinaryOperator::GreaterOrEqual => {
                        let result = if lhs >= rhs { 1 } else { 0 };
                        Ok(Value::int(result, 1, lhs_s))
                    }
                    _ => todo!("will be covered all operator"),
                }
            }
            _ => todo!(),
        }
    }

    pub fn calculate_unary_operator_expression(
        op: &ast::UnaryOperator,
        operand: Value,
    ) -> Result<Value, ()> {
        match (op, operand) {
            (
                ast::UnaryOperator::Plus,
                Value::Int {
                    value,
                    width,
                    is_signed,
                },
            ) => Ok(Value::int(value, width, is_signed)),
            (
                ast::UnaryOperator::Minus,
                Value::Int {
                    value,
                    width,
                    is_signed,
                },
            ) => {
                assert!(is_signed);
                let result = -(value as i128);
                Ok(Value::int(result as u128, width, is_signed))
            }
            (
                ast::UnaryOperator::Negate,
                Value::Int {
                    value,
                    width,
                    is_signed,
                },
            ) => {
                // Check if it is boolean
                assert!(width == 1);
                let result = if value == 0 { 1 } else { 0 };
                Ok(Value::int(result, width, is_signed))
            }
            _ => todo!(),
        }
    }

    pub fn calculate_typecast(value: Value, dtype: crate::ir::Dtype) -> Result<Value, ()> {
        match (value, dtype) {
            // TODO: distinguish zero/signed extension in the future
            // TODO: consider truncate in the future
            (
                Value::Int { value, .. },
                crate::ir::Dtype::Int {
                    width, is_signed, ..
                },
            ) => Ok(Value::int(value, width, is_signed)),
            (Value::Float { value, .. }, crate::ir::Dtype::Float { width, .. }) => {
                Ok(Value::float(value, width))
            }
            (value, dtype) => todo!("calculate_typecast ({:?}) {:?}", dtype, value),
        }
    }
}

#[derive(Default, Debug, PartialEq)]
struct Memory {
    // TODO: memory type should change to Vec<Vec<Byte>>
    inner: Vec<Vec<Value>>,
}

impl Memory {
    fn alloc(&mut self, dtype: &Dtype) -> Result<usize, InterpreterError> {
        let memory_block = match dtype {
            ir::Dtype::Unit { .. }
            | ir::Dtype::Int { .. }
            | ir::Dtype::Float { .. }
            | ir::Dtype::Pointer { .. } => vec![Value::default_from_dtype(dtype)],
            ir::Dtype::Function { .. } => vec![],
        };

        self.inner.push(memory_block);

        Ok(self.inner.len() - 1)
    }

    fn load(&self, bid: usize, offset: usize) -> &Value {
        &self.inner[bid][offset]
    }

    fn store(&mut self, bid: usize, offset: usize, value: Value) {
        self.inner[bid][offset] = value;
    }
}

// TODO: allocation fields will be added in the future
// TODO: program fields will be added in the future
#[derive(Debug, PartialEq)]
struct State<'i> {
    /// A data structure that maps each global variable to a pointer value
    /// When function call occurs, `registers` can be initialized by `global_registers`
    pub global_map: GlobalMap,
    pub stack_frame: StackFrame<'i>,
    pub stack: Vec<StackFrame<'i>>,
    pub memory: Memory,
    pub ir: &'i TranslationUnit,
}

impl<'i> State<'i> {
    fn new(ir: &'i TranslationUnit, args: Vec<Value>) -> Result<State, InterpreterError> {
        // Interpreter starts with the main function
        let func_name = String::from("main");
        let func = ir
            .decls
            .get(&func_name)
            .ok_or_else(|| InterpreterError::NoMainFunction)?;
        let (_, func_def) = func
            .get_function()
            .ok_or_else(|| InterpreterError::NoMainFunction)?;
        let func_def = func_def
            .as_ref()
            .ok_or_else(|| InterpreterError::NoFunctionDefinition {
                func_name: func_name.clone(),
            })?;

        // Create State
        let mut state = State {
            global_map: GlobalMap::default(),
            stack_frame: StackFrame::new(func_def.bid_init, func_name, func_def),
            stack: Vec::new(),
            memory: Default::default(),
            ir,
        };

        state.alloc_global_variables()?;

        // Initialize state with main function and args
        state.write_args(func_def.bid_init, args)?;
        state.alloc_local_variables()?;

        Ok(state)
    }

    fn alloc_global_variables(&mut self) -> Result<(), InterpreterError> {
        for (name, decl) in &self.ir.decls {
            // Memory allocation
            let bid = self.memory.alloc(&decl.dtype())?;
            self.global_map.insert(name.clone(), bid)?;

            // Initialize allocated memory space
            match decl {
                Declaration::Variable { dtype, initializer } => {
                    if dtype.get_function_inner().is_some() {
                        panic!("function variable does not exist")
                    }

                    if let Some(constant) = initializer {
                        let value = self.interp_constant(constant.clone());
                        self.memory.store(bid, 0, value);
                    }
                }
                // If functin declaration, skip initialization
                Declaration::Function { .. } => (),
            }
        }

        Ok(())
    }

    fn alloc_local_variables(&mut self) -> Result<(), InterpreterError> {
        // add alloc register
        for (id, allocation) in self.stack_frame.func_def.allocations.iter().enumerate() {
            let bid = self.memory.alloc(&allocation)?;
            let ptr = Value::pointer(Some(bid), 0);
            let rid = RegisterId::local("".to_string(), id);

            self.stack_frame.registers.write(rid, ptr)
        }

        Ok(())
    }

    fn write_args(&mut self, bid_init: BlockId, args: Vec<Value>) -> Result<(), InterpreterError> {
        for (i, value) in args.iter().enumerate() {
            self.stack_frame
                .registers
                .write(RegisterId::arg(bid_init, i), value.clone());
        }

        Ok(())
    }

    fn step(&mut self) -> Result<Option<Value>, InterpreterError> {
        let block = self
            .stack_frame
            .func_def
            .blocks
            .get(&self.stack_frame.pc.bid)
            .expect("block matched with `bid` must be exist");

        // If it's time to execute an instruction, do so.
        if let Some(instr) = block.instructions.get(self.stack_frame.pc.iid) {
            self.interp_instruction(instr)?;
            return Ok(None);
        }

        // Execute a block exit.
        let return_value = some_or!(self.interp_block_exit(&block.exit)?, return Ok(None));

        // If it's returning from a function, pop the stack frame.

        // TODO: free memory allocated in the callee

        // restore previous state
        let prev_stack_frame = some_or!(self.stack.pop(), return Ok(Some(return_value)));
        self.stack_frame = prev_stack_frame;

        // create temporary register to write return value
        let register = RegisterId::temp(self.stack_frame.pc.bid, self.stack_frame.pc.iid);
        self.stack_frame.registers.write(register, return_value);
        self.stack_frame.pc.increment();
        Ok(None)
    }

    fn run(&mut self) -> Result<Value, InterpreterError> {
        loop {
            if let Some(value) = self.step()? {
                return Ok(value);
            }
        }
    }

    fn interp_args(
        &self,
        signature: &FunctionSignature,
        args: &[Operand],
    ) -> Result<Vec<Value>, InterpreterError> {
        // Check that the dtype of each args matches the expected
        if !(args.len() == signature.params.len()
            && izip!(args, &signature.params).all(|(a, d)| a.dtype().is_compatible(d)))
        {
            panic!("dtype of args and params must be compatible")
        }

        args.iter()
            .map(|a| self.interp_operand(a.clone()))
            .collect::<Result<Vec<_>, _>>()
    }

    fn interp_jump(&mut self, arg: &JumpArg) -> Result<Option<Value>, InterpreterError> {
        let block = self
            .stack_frame
            .func_def
            .blocks
            .get(&arg.bid)
            .expect("block matched with `arg.bid` must be exist");

        assert_eq!(arg.args.len(), block.phinodes.len());
        for (a, d) in izip!(&arg.args, &block.phinodes) {
            assert!(a.dtype().is_compatible(&d));
        }

        for (i, a) in arg.args.iter().enumerate() {
            let v = self.interp_operand(a.clone()).unwrap();
            self.stack_frame
                .registers
                .inner
                .insert(RegisterId::arg(arg.bid, i), v)
                .unwrap();
        }

        self.stack_frame.pc = Pc::new(arg.bid);
        Ok(None)
    }

    fn interp_block_exit(
        &mut self,
        block_exit: &BlockExit,
    ) -> Result<Option<Value>, InterpreterError> {
        match block_exit {
            BlockExit::Jump { arg } => self.interp_jump(arg),
            BlockExit::ConditionalJump {
                condition,
                arg_then,
                arg_else,
            } => {
                let value = self.interp_operand(condition.clone())?;
                let (value, width, _) = value.get_int().expect("`condition` must be `Value::Int`");
                // Check if it is boolean
                assert!(width == 1);

                self.interp_jump(if value == 1 { arg_then } else { arg_else })
            }
            BlockExit::Switch {
                value,
                default,
                cases,
            } => {
                let value = self.interp_operand(value.clone())?;

                // TODO: consider different integer `width` in the future
                let arg = cases
                    .iter()
                    .find(|(c, _)| value == self.interp_constant(c.clone()))
                    .map(|(_, arg)| arg)
                    .unwrap_or_else(|| default);
                self.interp_jump(arg)
            }
            BlockExit::Return { value } => Ok(Some(self.interp_operand(value.clone())?)),
            BlockExit::Unreachable => Err(InterpreterError::Unreachable),
        }
    }

    fn interp_instruction(&mut self, instruction: &Instruction) -> Result<(), InterpreterError> {
        let result = match instruction {
            Instruction::BinOp { op, lhs, rhs, .. } => {
                let lhs = self.interp_operand(lhs.clone())?;
                let rhs = self.interp_operand(rhs.clone())?;

                calculator::calculate_binary_operator_expression(&op, lhs, rhs).map_err(|_| {
                    InterpreterError::Misc {
                        func_name: self.stack_frame.func_name.clone(),
                        pc: self.stack_frame.pc,
                        msg: "calculate_binary_operator_expression".into(),
                    }
                })?
            }
            Instruction::UnaryOp { op, operand, .. } => {
                let operand = self.interp_operand(operand.clone())?;

                calculator::calculate_unary_operator_expression(&op, operand).map_err(|_| {
                    InterpreterError::Misc {
                        func_name: self.stack_frame.func_name.clone(),
                        pc: self.stack_frame.pc,
                        msg: "calculate_unary_operator_expression".into(),
                    }
                })?
            }
            Instruction::Store { ptr, value, .. } => {
                let ptr = self.interp_operand(ptr.clone())?;
                let value = self.interp_operand(value.clone())?;
                let (bid, offset) = self.interp_ptr(ptr)?;
                self.memory.store(bid, offset, value);

                Value::Unit
            }
            Instruction::Load { ptr, .. } => {
                let ptr = self.interp_operand(ptr.clone())?;
                let (bid, offset) = self.interp_ptr(ptr)?;
                self.memory.load(bid, offset).clone()
            }
            Instruction::Call { callee, args, .. } => {
                let ptr = self.interp_operand(callee.clone())?;

                // Get function name from pointer
                let (bid, _) = ptr.get_pointer().expect("`ptr` must be `Value::Pointer`");
                let bid = bid.expect("pointer for global variable must have bid value");
                let callee_name = self
                    .global_map
                    .get_var(bid)
                    .expect("bid must have relation with global variable");

                let func = self
                    .ir
                    .decls
                    .get(&callee_name)
                    .expect("function must be declared before being called");
                let (func_signature, func_def) = func
                    .get_function()
                    .expect("`func` must be function declaration");
                let func_def =
                    func_def
                        .as_ref()
                        .ok_or_else(|| InterpreterError::NoFunctionDefinition {
                            func_name: callee_name.clone(),
                        })?;

                let args = self.interp_args(func_signature, args)?;

                let stack_frame = StackFrame::new(func_def.bid_init, callee_name, func_def);
                let prev_stack_frame = mem::replace(&mut self.stack_frame, stack_frame);
                self.stack.push(prev_stack_frame);

                // Initialize state with function obtained by callee and args
                self.write_args(func_def.bid_init, args)?;
                self.alloc_local_variables()?;

                return Ok(());
            }
            Instruction::TypeCast {
                value,
                target_dtype,
            } => {
                let value = self.interp_operand(value.clone())?;
                calculator::calculate_typecast(value, target_dtype.clone()).map_err(|_| {
                    InterpreterError::Misc {
                        func_name: self.stack_frame.func_name.clone(),
                        pc: self.stack_frame.pc,
                        msg: "calculate_typecast".into(),
                    }
                })?
            }
        };

        let register = RegisterId::temp(self.stack_frame.pc.bid, self.stack_frame.pc.iid);
        self.stack_frame.registers.write(register, result);
        self.stack_frame.pc.increment();

        Ok(())
    }

    fn interp_operand(&self, operand: Operand) -> Result<Value, InterpreterError> {
        match &operand {
            Operand::Constant(value) => Ok(self.interp_constant(value.clone())),
            Operand::Register { rid, .. } => {
                Ok(self.stack_frame.registers.read(rid.clone()).clone())
            }
        }
    }

    fn interp_constant(&self, value: Constant) -> Value {
        match value {
            Constant::Unit => Value::Unit,
            Constant::Int {
                value,
                width,
                is_signed,
            } => Value::Int {
                value,
                width,
                is_signed,
            },
            Constant::Float { value, width } => Value::Float { value, width },
            Constant::GlobalVariable { name, .. } => {
                let bid = self
                    .global_map
                    .get_bid(&name)
                    .expect("The name matching `bid` must exist.");

                // Generate appropriate pointer from `bid`
                Value::Pointer {
                    bid: Some(bid),
                    offset: 0,
                }
            }
        }
    }

    fn interp_ptr(&mut self, pointer: Value) -> Result<(usize, usize), InterpreterError> {
        let (bid, offset) = pointer
            .get_pointer()
            .ok_or_else(|| InterpreterError::Misc {
                func_name: self.stack_frame.func_name.clone(),
                pc: self.stack_frame.pc,
                msg: "Accessing memory with non-pointer".into(),
            })?;

        let bid = bid.ok_or_else(|| InterpreterError::Misc {
            func_name: self.stack_frame.func_name.clone(),
            pc: self.stack_frame.pc,
            msg: "Accessing memory with constant pointer".into(),
        })?;

        Ok((bid, offset))
    }
}

#[inline]
pub fn interp(ir: &TranslationUnit, args: Vec<Value>) -> Result<Value, InterpreterError> {
    let mut init_state = State::new(ir, args)?;
    init_state.run()
}
