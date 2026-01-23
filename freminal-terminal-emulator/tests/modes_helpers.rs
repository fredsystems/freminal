// Copyright (C) 2024-2026 Fred Clausen
// MIT license.

use freminal_common::buffer_states::{
    mode::SetMode,
    modes::{decawm::Decawm, deccolm::Deccolm, decom::Decom, dectcem::Dectcem, lnm::Lnm},
};

#[test]
fn decawm_modes_toggle_correctly() {
    let m_on = Decawm::new(&SetMode::DecSet);
    let m_off = Decawm::new(&SetMode::DecRst);
    println!("DECAWM on {:?} off {:?}", m_on, m_off);
    assert_ne!(m_on, m_off);
    assert_eq!(format!("{}", m_on), "Autowrap Mode (DECAWM) Enabled");
    assert_eq!(format!("{}", m_off), "Autowrap Mode (DECAWM) Disabled");
}

#[test]
fn dectcem_visibility_modes() {
    let visible = Dectcem::new(&SetMode::DecSet);
    let invisible = Dectcem::new(&SetMode::DecRst);
    println!("DECTCEM visible {:?} invisible {:?}", visible, invisible);
    assert_ne!(visible, invisible);
}

#[test]
fn deccolm_columns_mode() {
    let wide = Deccolm::new(&SetMode::DecSet);
    let normal = Deccolm::new(&SetMode::DecRst);
    println!("DECCOLM wide {:?} normal {:?}", wide, normal);
    assert_ne!(wide, normal);
}

#[test]
fn decom_origin_mode() {
    let relative = Decom::new(&SetMode::DecSet);
    let absolute = Decom::new(&SetMode::DecRst);
    println!("DECOM relative {:?} absolute {:?}", relative, absolute);
    assert_ne!(relative, absolute);
}

#[test]
fn lnm_linefeed_mode() {
    let enable = Lnm::new(&SetMode::DecSet);
    let disable = Lnm::new(&SetMode::DecRst);
    println!("LNM enable {:?} disable {:?}", enable, disable);
    assert_ne!(enable, disable);
}
