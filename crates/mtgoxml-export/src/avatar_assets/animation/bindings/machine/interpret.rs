//! Evaluate typed IL instructions without executing managed code.

use super::*;

impl Machine<'_> {
    /// Evaluate normalized IL with fixed recursion, instruction, heap and branch bounds.
    pub(super) fn run(
        &mut self,
        method: &Method,
        arguments: Vec<Value>,
        depth: usize,
    ) -> Result<Value> {
        if depth > 64 || method.has_exception_handlers {
            return Err(format!(
                "unsupported binding control flow in {}::{}",
                method.owner, method.name
            )
            .into());
        }
        let arguments: Vec<_> = arguments
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect();
        let mut locals: BTreeMap<u16, Rc<RefCell<Value>>> = BTreeMap::new();
        let mut stack = Vec::<Value>::new();
        let mut pc = 0;
        while let Some(instruction) = method.body.get(pc) {
            // Consume an instruction within the stack, heap and fuel bounds.
            if self.fuel == 0
                || stack.len() > 1024
                || self.objects.len() > 4096
                || self.arrays.len() > 4096
            {
                return Err("binding extraction exceeds its evaluation limits".into());
            }
            self.fuel -= 1;
            pc += 1;
            let pop = |stack: &mut Vec<Value>| {
                stack.pop().ok_or_else(|| {
                    format!(
                        "IL stack underflow in {}::{} at {}",
                        method.owner, method.name, instruction.offset
                    )
                    .into()
                })
            };
            match instruction.opcode {
                OpCode::Nop
                | OpCode::Constrained
                | OpCode::Readonly
                | OpCode::Volatile
                | OpCode::Tail => {}
                OpCode::Ldnull => stack.push(Value::Null),
                OpCode::LdcI4 | OpCode::LdcI8 => {
                    if let Operand::Integer(value) = instruction.operand {
                        stack.push(Value::Integer(value));
                    } else {
                        return Err("invalid IL integer operand".into());
                    }
                }
                OpCode::LdcR4 | OpCode::LdcR8 => {
                    if let Operand::Float(value) = instruction.operand {
                        stack.push(Value::Number(value));
                    } else {
                        return Err("invalid IL floating operand".into());
                    }
                }
                OpCode::Ldstr => {
                    if let Operand::String(value) = &instruction.operand {
                        stack.push(Value::Text(value.clone()));
                    } else {
                        return Err("invalid IL string operand".into());
                    }
                }
                OpCode::Ldarg | OpCode::Ldarga => {
                    if let Operand::Variable(index) = instruction.operand {
                        let value = arguments
                            .get(index as usize)
                            .ok_or("IL argument index exceeds signature")?;
                        stack.push(if instruction.opcode == OpCode::Ldarga {
                            Value::Address(value.clone())
                        } else {
                            value.borrow().clone()
                        });
                    }
                }
                OpCode::Ldloc | OpCode::Ldloca | OpCode::Stloc => {
                    if let Operand::Variable(index) = instruction.operand {
                        let value = locals
                            .entry(index)
                            .or_insert_with(|| Rc::new(RefCell::new(Value::Unknown)));
                        if instruction.opcode == OpCode::Stloc {
                            *value.borrow_mut() = pop(&mut stack)?;
                        } else {
                            stack.push(if instruction.opcode == OpCode::Ldloca {
                                Value::Address(value.clone())
                            } else {
                                value.borrow().clone()
                            });
                        }
                    }
                }
                OpCode::Dup => {
                    let value = stack.last().ok_or("IL dup on an empty stack")?.clone();
                    stack.push(value);
                }
                OpCode::Pop => {
                    pop(&mut stack)?;
                }
                OpCode::Ldtoken => match &instruction.operand {
                    Operand::Type(ty) => stack.push(Value::Type(ty.clone())),
                    _ => return Err("unsupported binding metadata token".into()),
                },
                OpCode::Ldftn => match &instruction.operand {
                    Operand::Method(method) => stack.push(Value::Method(method.clone())),
                    _ => return Err("invalid binding callback token".into()),
                },
                OpCode::Ldsfld | OpCode::Stsfld | OpCode::Ldfld | OpCode::Stfld => {
                    // Resolve the original field identity before applying its access semantics.
                    let Operand::Field(field) = &instruction.operand else {
                        return Err("invalid IL field operand".into());
                    };
                    let key = Self::field(field);
                    match instruction.opcode {
                        OpCode::Ldsfld => {
                            self.initialize(&field.owner, depth + 1)?;
                            stack.push(self.fields.get(&key).cloned().unwrap_or_else(|| {
                                if self.ty(&field.owner).is_some() {
                                    default_value(&field.ty)
                                } else {
                                    Value::Unknown
                                }
                            }));
                        }
                        OpCode::Stsfld => {
                            // Attach the original field identity to a stored dependency-property declaration.
                            let value = pop(&mut stack)?;
                            if let Value::Property(index) = value {
                                self.properties[index].field = key.clone();
                            }
                            self.fields.insert(key, value);
                        }
                        OpCode::Ldfld => {
                            let object = pop(&mut stack)?.object()?;
                            stack.push(
                                self.objects[object]
                                    .fields
                                    .get(&key)
                                    .cloned()
                                    .unwrap_or_else(|| default_value(&field.ty)),
                            );
                        }
                        _ => {
                            // Retain the interpreted value on its modeled receiver.
                            let value = pop(&mut stack)?;
                            let object = pop(&mut stack)?.object()?;
                            self.objects[object].fields.insert(key, value);
                        }
                    }
                }
                OpCode::Call | OpCode::Callvirt | OpCode::Newobj => {
                    // Consume the exact call signature and propagate only a declared result.
                    let Operand::Method(target) = &instruction.operand else {
                        return Err("invalid IL call operand".into());
                    };
                    let constructor = instruction.opcode == OpCode::Newobj;
                    let count =
                        target.parameter_count + usize::from(target.has_this && !constructor);
                    if stack.len() < count {
                        return Err(format!(
                            "IL call stack underflow for {}::{}",
                            target.owner, target.name
                        )
                        .into());
                    }
                    let args = stack.split_off(stack.len() - count);
                    let value = self
                        .call(
                            target,
                            args,
                            constructor,
                            instruction.opcode == OpCode::Callvirt,
                            depth + 1,
                        )
                        .map_err(|error| {
                            format!(
                                "{error} (from {}::{} at IL_{:04x})",
                                method.owner, method.name, instruction.offset
                            )
                        })?;
                    if constructor || target.returns_value {
                        stack.push(value);
                    }
                }
                OpCode::Box => {
                    // Keep boxed zero values nonnull while retaining their managed type.
                    let value = pop(&mut stack)?;
                    let Operand::Type(ty) = &instruction.operand else {
                        return Err("invalid boxed IL type".into());
                    };
                    stack.push(Value::Boxed(Box::new(value), ty.clone()));
                }
                OpCode::UnboxAny => {
                    let value = pop(&mut stack)?;
                    stack.push(if let Value::Boxed(value, _) = value {
                        *value
                    } else {
                        value
                    });
                }
                OpCode::Castclass => {}
                OpCode::ConvR8
                | OpCode::ConvR4
                | OpCode::ConvI4
                | OpCode::ConvU4
                | OpCode::ConvI8
                | OpCode::ConvU8 => {
                    let value = pop(&mut stack)?;
                    stack.push(convert(instruction.opcode, value)?);
                }
                OpCode::Isinst => {
                    // Preserve concrete object and boxed-value identity during conditional casts.
                    let value = pop(&mut stack)?;
                    let Operand::Type(ty) = &instruction.operand else {
                        return Err("invalid IL type operand".into());
                    };
                    let valid = match &value {
                        Value::Object(index) => self.inherits(&self.objects[*index].ty, ty),
                        Value::Boxed(_, boxed_type) => boxed_type == ty,
                        _ => false,
                    };
                    stack.push(if valid { value } else { Value::Null });
                }
                OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::Div
                | OpCode::Rem
                | OpCode::And
                | OpCode::Or
                | OpCode::Xor
                | OpCode::Ceq
                | OpCode::Cgt
                | OpCode::CgtUn
                | OpCode::Clt
                | OpCode::CltUn => {
                    // Apply CLI arithmetic to the stack operands without host-language coercion.
                    let right = pop(&mut stack)?;
                    let left = pop(&mut stack)?;
                    stack.push(binary(instruction.opcode, left, right)?);
                }
                OpCode::Neg => {
                    let value = pop(&mut stack)?.number()?;
                    stack.push(Value::Number(-value));
                }
                OpCode::Br
                | OpCode::Brtrue
                | OpCode::Brfalse
                | OpCode::Beq
                | OpCode::BneUn
                | OpCode::Bgt
                | OpCode::BgtUn
                | OpCode::Bge
                | OpCode::BgeUn
                | OpCode::Blt
                | OpCode::BltUn
                | OpCode::Ble
                | OpCode::BleUn => {
                    let take = match instruction.opcode {
                        OpCode::Br => true,
                        OpCode::Brtrue => pop(&mut stack)?.truth()?,
                        OpCode::Brfalse => !pop(&mut stack)?.truth()?,
                        name => {
                            // Evaluate the concrete condition and jump only to an instruction boundary.
                            let right = pop(&mut stack)?;
                            let left = pop(&mut stack)?;
                            if matches!(
                                name,
                                OpCode::BgtUn | OpCode::BgeUn | OpCode::BltUn | OpCode::BleUn
                            ) {
                                unsigned_operands(&left, &right)?;
                            }
                            let comparison = compare(&left, &right)?;
                            match name {
                                OpCode::Beq => comparison == 0,
                                OpCode::BneUn => comparison != 0,
                                OpCode::Bgt | OpCode::BgtUn => comparison > 0,
                                OpCode::Bge | OpCode::BgeUn => comparison >= 0,
                                OpCode::Blt | OpCode::BltUn => comparison < 0,
                                _ => comparison <= 0,
                            }
                        }
                    };
                    if take {
                        let Operand::Branch(offset) = instruction.operand else {
                            return Err("invalid IL branch operand".into());
                        };
                        pc = target_index(method, offset)?;
                    }
                }
                OpCode::Switch => {
                    // Select the decoded target using the concrete switch discriminator.
                    let value = pop(&mut stack)?.integer()?;
                    let Operand::Switch(offsets) = &instruction.operand else {
                        return Err("invalid IL switch operand".into());
                    };
                    if let Some(offset) = usize::try_from(value)
                        .ok()
                        .and_then(|index| offsets.get(index))
                    {
                        pc = target_index(method, *offset)?;
                    }
                }
                OpCode::Newarr => {
                    // Bound this array and aggregate allocations before reserving elements.
                    let count = usize::try_from(pop(&mut stack)?.integer()?)?;
                    self.array_elements = self
                        .array_elements
                        .checked_add(count)
                        .ok_or("binding IL array allocation overflow")?;
                    if count > 4096 || self.array_elements > 65_536 {
                        return Err("binding IL array is oversized".into());
                    }
                    let index = self.arrays.len();
                    self.arrays.push(vec![Value::Null; count]);
                    stack.push(Value::Array(index));
                }
                OpCode::StelemRef | OpCode::StelemI2 | OpCode::StelemI4 => {
                    // Bounds-check the modeled array slot before storing its value.
                    let value = pop(&mut stack)?;
                    let index = usize::try_from(pop(&mut stack)?.integer()?)?;
                    let Value::Array(array) = pop(&mut stack)? else {
                        return Err("invalid IL array store".into());
                    };
                    *self
                        .arrays
                        .get_mut(array)
                        .and_then(|array| array.get_mut(index))
                        .ok_or("IL array store index exceeds its bound")? = value;
                }
                OpCode::LdelemRef | OpCode::LdelemI2 | OpCode::LdelemI4 => {
                    // Read the existing modeled array element with its retained value.
                    let index = usize::try_from(pop(&mut stack)?.integer()?)?;
                    let Value::Array(array) = pop(&mut stack)? else {
                        return Err("invalid IL array load".into());
                    };
                    stack.push(
                        self.arrays
                            .get(array)
                            .and_then(|array| array.get(index))
                            .ok_or("IL array load index exceeds its bound")?
                            .clone(),
                    );
                }
                OpCode::Initobj => {
                    // Initialize the addressed managed value using CLR zero defaults.
                    let Value::Address(value) = pop(&mut stack)? else {
                        return Err("unsupported IL initialization target".into());
                    };
                    let Operand::Type(ty) = &instruction.operand else {
                        return Err("invalid IL initialization type".into());
                    };
                    *value.borrow_mut() = default_value(ty);
                }
                OpCode::Ret => {
                    return if method.returns_value {
                        pop(&mut stack)
                    } else {
                        Ok(Value::Null)
                    };
                }
                OpCode::Throw => {
                    return Err(format!(
                        "captured client rejects avatar premium effect {}",
                        self.effect
                    )
                    .into());
                }
                other => {
                    return Err(format!(
                        "unsupported binding IL {other:?} in {}::{} at {}",
                        method.owner, method.name, instruction.offset
                    )
                    .into());
                }
            }
        }
        Err(format!("IL method {}::{} has no return", method.owner, method.name).into())
    }
}
