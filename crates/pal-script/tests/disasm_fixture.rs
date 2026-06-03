use pal_script::{
    disassemble_script, format_script_header, DisassembleOptions, Instruction, PointTable,
    ScriptImage,
};

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

#[test]
fn parses_script_header() {
    let mut script = Vec::new();
    script.extend_from_slice(b"Sv20");
    push_u32(&mut script, 0x1122_3344);
    push_u32(&mut script, 0x0000_000C);

    let parsed = ScriptImage::parse(&script).unwrap();
    assert_eq!(parsed.check_value(), 0x1122_3344);
    assert_eq!(parsed.entry_pc(), 0x0000_000C);
    assert_eq!(
        format_script_header(&parsed),
        "# magic=Sv20 check=0x11223344 code_base=0x0000000C entry=0x0000000C size=0x0000000C"
    );
}

#[test]
fn disassembles_mov_and_end() {
    let mut script = Vec::new();
    script.extend_from_slice(b"Sv20");
    push_u32(&mut script, 0);
    push_u32(&mut script, 0x0000_000C);
    push_u32(&mut script, 0x0001_0001);
    push_u32(&mut script, 0x4000_0005);
    push_u32(&mut script, 0x0000_007B);
    push_u32(&mut script, 0x0001_0015);

    let parsed = ScriptImage::parse(&script).unwrap();
    let instructions =
        disassemble_script(&parsed, DisassembleOptions::new(parsed.entry_pc() as usize)).unwrap();

    assert_eq!(instructions.len(), 2);
    assert_eq!(
        instructions[0].to_string(),
        "0000000C: mov var[5], imm(123) ; word=0x00010001"
    );
    assert_eq!(
        instructions[1].to_string(),
        "00000018: end ; word=0x00010015"
    );
}

#[test]
fn resolves_reverse_point_table_indices() {
    let mut point_bytes = Vec::new();
    push_u32(&mut point_bytes, 0x0000_0100);
    push_u32(&mut point_bytes, 0x0000_0200);
    let points = PointTable::parse(&point_bytes).unwrap();

    assert_eq!(points.resolve_target_pc(1).unwrap(), Some(0x0000_020C));
    assert_eq!(points.resolve_target_pc(2).unwrap(), Some(0x0000_010C));
    assert_eq!(points.resolve_target_pc(0).unwrap(), None);
}

#[test]
fn disassembles_jump_with_point_target() {
    let mut script = Vec::new();
    script.extend_from_slice(b"Sv20");
    push_u32(&mut script, 0);
    push_u32(&mut script, 0x0000_000C);
    push_u32(&mut script, 0x0001_0009);
    push_u32(&mut script, 1);

    let mut point_bytes = Vec::new();
    push_u32(&mut point_bytes, 0x0000_0020);
    let points = PointTable::parse(&point_bytes).unwrap();
    let parsed = ScriptImage::parse(&script).unwrap();
    let mut options = DisassembleOptions::new(parsed.entry_pc() as usize);
    options.point_table = Some(&points);

    let instructions = disassemble_script(&parsed, options).unwrap();
    match &instructions[0] {
        Instruction::Primary { opcode, .. } => assert_eq!(*opcode, 9),
        _ => panic!("expected primary instruction"),
    }
    assert_eq!(
        instructions[0].to_string(),
        "0000000C: jmp_point point[1] -> 0x0000002C ; word=0x00010009"
    );
}
