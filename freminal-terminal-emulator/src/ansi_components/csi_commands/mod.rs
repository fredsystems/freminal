// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! CSI command sub-handlers.
//!
//! Each module implements one (or a small family of) CSI sequence(s).
//! Routing lives in `csi_dispatch.rs`: `AnsiCsiParser::ansiparser_inner_csi`
//! classifies the sequence into a `CsiKey` (prefix, intermediate, final byte)
//! and calls `csi_dispatch::dispatch_csi`, which calls the handler here.
//! Routing is strict: the whole key must match a row below. A key with valid
//! grammar but no row is recognised and unhandled (logged, no output).
//!
//! ## Dispatch table
//!
//! "Prefix" is the private-marker parameter byte (`?`, `>`, `<`, `=`);
//! "Intermediate" is the intermediate byte (`SP`, `!`, `$`). Both are "none"
//! when absent. Handlers still receive the parameter string with the prefix
//! byte included.
//!
//! | Prefix | Intermediate | Final         | Mnemonic                     | Handler module      |
//! |--------|--------------|---------------|------------------------------|---------------------|
//! | none   | none         | `A`           | CUU                          | `cuu`               |
//! | none   | none         | `B`           | CUD                          | `cud`               |
//! | none   | none         | `C`           | CUF                          | `cuf`               |
//! | none   | none         | `D`           | CUB                          | `cub`               |
//! | none   | none         | `E`           | CNL                          | `cnl`               |
//! | none   | none         | `F`           | CPL                          | `cpl`               |
//! | none   | none         | `G` / `` ` `` | CHA / HPA                    | `cha`               |
//! | none   | none         | `H` / `f`     | CUP / HVP                    | `cup`               |
//! | none   | none         | `I`           | CHT                          | `cht`               |
//! | none   | none         | `J`           | ED                           | `ed`                |
//! | none   | none         | `K`           | EL                           | `el`                |
//! | none   | none         | `L`           | IL                           | `il`                |
//! | none   | none         | `M`           | DL                           | `dl`                |
//! | none   | none         | `P`           | DCH                          | `dch`               |
//! | none   | none         | `S`           | SU                           | `su`                |
//! | none   | none         | `T`           | SD                           | `sd`                |
//! | none   | none         | `X`           | ECH                          | `ech`               |
//! | none   | none         | `Z`           | CBT                          | `cbt`               |
//! | none   | none         | `@`           | ICH                          | `ich`               |
//! | none   | none         | `b`           | REP                          | `rep`               |
//! | none   | none         | `d`           | VPA                          | `vpa`               |
//! | none   | none         | `g`           | TBC                          | `tbc`               |
//! | none   | none         | `m`           | SGR                          | `sgr`               |
//! | none   | none         | `h` / `l`     | SM / RM (mode set / reset)   | `dec_modes`         |
//! | none   | none         | `n`           | DSR                          | `dsr`               |
//! | none   | none         | `c`           | DA1                          | `da`                |
//! | none   | none         | `r`           | DECSTBM                      | `decstbm`           |
//! | none   | none         | `s`           | DECSLRM (SCOSC if no params) | `decslrm`           |
//! | none   | none         | `u`           | SCORC                        | `scorc`             |
//! | none   | none         | `t`           | DECSLPP / XTWINOPS           | `decslpp`           |
//! | none   | none         | `x`           | DECREQTPARM                  | `decreqtparm`       |
//! | none   | `$`          | `p`           | DECRQM (ANSI mode)           | `decrqm`            |
//! | none   | `!`          | `p`           | DECSTR                       | `decstr`            |
//! | none   | SP           | `q`           | DECSCUSR                     | `decscusr`          |
//! | `?`    | none         | `h` / `l`     | DECSET / DECRST              | `dec_modes`         |
//! | `?`    | none         | `n`           | DEC private DSR              | `dsr`               |
//! | `?`    | none         | `u`           | Kitty keyboard: query        | `scorc`             |
//! | `?`    | `$`          | `p`           | DECRQM (DEC private mode)    | `decrqm`            |
//! | `>`    | none         | `c`           | DA2                          | `da`                |
//! | `>`    | none         | `q`           | XTVERSION                    | `xtversion`         |
//! | `>`    | none         | `m`           | XTMODKEYS (modifyOtherKeys)  | `modify_other_keys` |
//! | `>`    | none         | `u`           | Kitty keyboard: push         | `scorc`             |
//! | `<`    | none         | `u`           | Kitty keyboard: pop          | `scorc`             |
//! | `=`    | none         | `c`           | DA3                          | `da`                |
//! | `=`    | none         | `u`           | Kitty keyboard: set          | `scorc`             |

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
pub mod dec_modes;
pub mod decreqtparm;
pub mod decrqm;
pub mod decscusr;
pub mod decslpp;
pub mod decslrm;
pub mod decstbm;
pub mod decstr;
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
