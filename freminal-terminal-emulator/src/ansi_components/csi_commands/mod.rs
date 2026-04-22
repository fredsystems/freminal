// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! CSI command sub-handlers.
//!
//! Each module implements one (or a small family of) CSI sequence(s).
//! The dispatch entry point is `csi.rs`'s
//! `ansi_parser_inner_csi_finished`, which matches on the CSI final byte
//! (and optional intermediate byte) and calls the appropriate handler here.
//!
//! ## Dispatch table (final byte → module)
//!
//! | Final byte | Intermediate | Sequence    | Module        |
//! |------------|--------------|-------------|---------------|
//! | `A`        | —            | CUU         | `cuu`         |
//! | `B`        | —            | CUD         | `cud`         |
//! | `C`        | —            | CUF         | `cuf`         |
//! | `D`        | —            | CUB         | `cub`         |
//! | `E`        | —            | CNL         | `cnl`         |
//! | `F`        | —            | CPL         | `cpl`         |
//! | `G`        | —            | CHA         | `cha`         |
//! | `H` / `f`  | —            | CUP / HVP   | `cup`         |
//! | `I`        | —            | CHT         | `cht`         |
//! | `J`        | —            | ED          | `ed`          |
//! | `K`        | —            | EL          | `el`          |
//! | `L`        | —            | IL          | `il`          |
//! | `M`        | —            | DL          | `dl`          |
//! | `P`        | —            | DCH         | `dch`         |
//! | `S`        | —            | SU          | `su`          |
//! | `T`        | —            | SD          | `sd`          |
//! | `W`        | —            | TBC         | `tbc`         |
//! | `X`        | —            | ECH         | `ech`         |
//! | `Z`        | —            | CBT         | `cbt`         |
//! | `@`        | —            | ICH         | `ich`         |
//! | `c`        | —            | DA1         | `da`          |
//! | `c`        | `>`          | DA2         | `da`          |
//! | `d`        | —            | VPA         | `vpa`         |
//! | `h` / `l`  | `?`          | DECSET/RST  | *(csi.rs)*    |
//! | `m`        | —            | SGR         | `sgr`         |
//! | `n`        | —            | DSR         | `dsr`         |
//! | `p`        | `>`          | MODKEYS     | `modify_other_keys` |
//! | `q`        | `>`          | XTVERSION   | `xtversion`   |
//! | `r`        | —            | DECSTBM     | `decstbm`     |
//! | `s`        | —            | DECSLRM     | `decslrm`     |
//! | `s` / `u`  | —            | SCORC/SCRC  | `scorc`       |
//! | `t`        | —            | XTWINOPS    | *(csi.rs)*    |
//! | `~`        | —            | REP / misc  | `rep`         |
//! | `q`        | ` ` (SP)     | DECSCUSR    | `decscusr`    |
//! | `p`        | `$`          | DECSLPP     | `decslpp`     |
//! | `p`        | `$`+`?`      | DECRQM      | `decrqm`      |

pub mod cbt;
pub mod cha;
pub mod cht;
pub mod cnl;
pub mod cpl;
pub mod cub;
pub mod cud;
pub mod cuf;
pub mod cup;
pub mod cuu;
pub mod da;
pub mod dch;
pub mod decrqm;
pub mod decscusr;
pub mod decslpp;
pub mod decslrm;
pub mod decstbm;
pub mod dl;
pub mod dsr;
pub mod ech;
pub mod ed;
pub mod el;
pub mod ich;
pub mod il;
pub mod modify_other_keys;
pub mod rep;
pub mod scorc;
pub mod sd;
pub mod sgr;
pub mod su;
pub mod tbc;
pub mod util;
pub mod vpa;
pub mod xtversion;
