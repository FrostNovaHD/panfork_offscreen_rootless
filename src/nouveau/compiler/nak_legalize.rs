/*
 * Copyright © 2022 Collabora, Ltd.
 * SPDX-License-Identifier: MIT
 */

use crate::nak_ir::*;

struct LegalizeInstr<'a> {
    ssa_alloc: &'a mut SSAValueAllocator,
    instrs: Vec<Instr>,
}

fn src_is_reg(src: &Src) -> bool {
    match src.src_ref {
        SrcRef::Zero | SrcRef::True | SrcRef::False | SrcRef::SSA(_) => true,
        SrcRef::Imm32(_) | SrcRef::CBuf(_) => false,
        SrcRef::Reg(_) => panic!("Not in SSA form"),
    }
}

fn src_as_lop_imm(src: &Src) -> Option<bool> {
    let x = match src.src_ref {
        SrcRef::Zero => false,
        SrcRef::True => true,
        SrcRef::False => false,
        SrcRef::Imm32(i) => {
            if i == 0 {
                false
            } else if i == !0 {
                true
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(x ^ src.src_mod.has_not())
}

fn fold_lop_src(src: &Src, x: &mut u8) {
    if let Some(i) = src_as_lop_imm(src) {
        *x = if i { !0 } else { 0 };
    }
}

impl<'a> LegalizeInstr<'a> {
    pub fn new(ssa_alloc: &'a mut SSAValueAllocator) -> LegalizeInstr {
        LegalizeInstr {
            ssa_alloc: ssa_alloc,
            instrs: Vec::new(),
        }
    }

    pub fn mov_src(&mut self, src: &mut Src, file: RegFile) {
        let val = self.ssa_alloc.alloc(file, 1);
        self.instrs
            .push(Instr::new_mov(val.into(), src.src_ref.into()));
        src.src_ref = val.into();
    }

    pub fn mov_src_if_not_reg(&mut self, src: &mut Src, file: RegFile) {
        if !src_is_reg(&src) {
            self.mov_src(src, file);
        }
    }

    pub fn swap_srcs_if_not_reg(&mut self, x: &mut Src, y: &mut Src) {
        if !src_is_reg(x) && src_is_reg(y) {
            std::mem::swap(x, y);
        }
    }

    pub fn map(&mut self, mut instr: Instr) -> Vec<Instr> {
        match &mut instr.op {
            Op::FAdd(op) => {
                let [ref mut src0, ref mut src1] = op.srcs;
                self.swap_srcs_if_not_reg(src0, src1);
                self.mov_src_if_not_reg(src0, RegFile::GPR);
            }
            Op::FFma(op) => {
                let [ref mut src0, ref mut src1, ref mut src2] = op.srcs;
                self.swap_srcs_if_not_reg(src0, src1);
                self.mov_src_if_not_reg(src0, RegFile::GPR);
                self.mov_src_if_not_reg(src2, RegFile::GPR);
            }
            Op::FMul(op) => {
                let [ref mut src0, ref mut src1] = op.srcs;
                self.swap_srcs_if_not_reg(src0, src1);
                self.mov_src_if_not_reg(src0, RegFile::GPR);
            }
            Op::IAdd3(op) => {
                let [ref mut src0, ref mut src1, ref mut src2] = op.srcs;
                self.swap_srcs_if_not_reg(src0, src1);
                self.swap_srcs_if_not_reg(src2, src1);
                self.mov_src_if_not_reg(src0, RegFile::GPR);
                self.mov_src_if_not_reg(src2, RegFile::GPR);
            }
            Op::IMad(op) => {
                let [ref mut src0, ref mut src1, ref mut src2] = op.srcs;
                self.swap_srcs_if_not_reg(src0, src1);
                self.mov_src_if_not_reg(src0, RegFile::GPR);
                self.mov_src_if_not_reg(src2, RegFile::GPR);
            }
            Op::Lop3(op) => {
                /* Fold constants if we can */
                op.op = LogicOp::new_lut(&|mut x, mut y, mut z| {
                    fold_lop_src(&op.srcs[0], &mut x);
                    fold_lop_src(&op.srcs[1], &mut y);
                    fold_lop_src(&op.srcs[2], &mut z);
                    op.op.eval(x, y, z)
                });
                for src in &mut op.srcs {
                    src.src_mod = SrcMod::None;
                    if src_as_lop_imm(src).is_some() {
                        src.src_ref = SrcRef::Zero;
                    }
                }

                let [ref mut src0, ref mut src1, ref mut src2] = op.srcs;
                if !src_is_reg(src0) && src_is_reg(src1) {
                    std::mem::swap(src0, src1);
                    op.op = LogicOp::new_lut(&|x, y, z| op.op.eval(y, x, z))
                }
                if !src_is_reg(src2) && src_is_reg(src1) {
                    std::mem::swap(src2, src1);
                    op.op = LogicOp::new_lut(&|x, y, z| op.op.eval(x, z, y))
                }

                self.mov_src_if_not_reg(src0, RegFile::GPR);
                self.mov_src_if_not_reg(src2, RegFile::GPR);
            }
            Op::PLop3(op) => {
                /* Fold constants if we can */
                for lop in &mut op.ops {
                    *lop = LogicOp::new_lut(&|mut x, mut y, mut z| {
                        fold_lop_src(&op.srcs[0], &mut x);
                        fold_lop_src(&op.srcs[1], &mut y);
                        fold_lop_src(&op.srcs[2], &mut z);
                        lop.eval(x, y, z)
                    });
                }
                for src in &mut op.srcs {
                    src.src_mod = SrcMod::None;
                    if src_as_lop_imm(src).is_some() {
                        src.src_ref = SrcRef::True;
                    }
                }

                let [ref mut src0, ref mut src1, ref mut src2] = op.srcs;
                if !src_is_reg(src0) && src_is_reg(src1) {
                    std::mem::swap(src0, src1);
                    for lop in &mut op.ops {
                        *lop = LogicOp::new_lut(&|x, y, z| lop.eval(y, x, z));
                    }
                }
                if !src_is_reg(src2) && src_is_reg(src1) {
                    std::mem::swap(src2, src1);
                    for lop in &mut op.ops {
                        *lop = LogicOp::new_lut(&|x, y, z| lop.eval(x, z, y));
                    }
                }

                self.mov_src_if_not_reg(src0, RegFile::GPR);
                self.mov_src_if_not_reg(src2, RegFile::GPR);
            }
            Op::St(op) => {
                self.mov_src_if_not_reg(&mut op.data, RegFile::GPR);
            }
            _ => (),
        }
        self.instrs.push(instr);
        std::mem::replace(&mut self.instrs, Vec::new())
    }
}

impl Shader {
    pub fn legalize(&mut self) {
        self.map_instrs(&|instr, ssa_alloc| -> Vec<Instr> {
            LegalizeInstr::new(ssa_alloc).map(instr)
        });
    }
}