library BuilderLib {
  struct RawCell {
    TvmCell data;
    uint16 ref0;
    uint16 ref1;
    uint16 ref2;
    uint16 ref3;
  }

  function upload(TvmCell raw_cells, mapping(uint16 => RawCell) chunk) internal {
    optional(uint16, RawCell) entry = chunk.max();
    while (entry.hasValue()) {
      (uint16 id, RawCell raw) = entry.get();
      TvmBuilder b;
      b.store(raw);
      b.storeRef(raw_cells);
      raw_cells = b.toCell();
      entry = chunk.prev(id);
    }
  }
  function finalize(TvmCell raw_cells) internal returns (TvmCell) {
    vector(TvmCell) fin_cells;
    TvmCell padding;
    fin_cells.push(padding);
    TvmSlice cursor = raw_cells.toSlice();
    while (cursor.refs() == 2) {
      RawCell cell = cursor.decode(RawCell);
      if (cell.ref0 != 0) {
        TvmBuilder b;
        b.store(cell.data.toSlice());
        b.storeRef(fin_cells[cell.ref0]);
        if (cell.ref1 != 0) {
          b.storeRef(fin_cells[cell.ref1]);
          if (cell.ref2 != 0) {
            b.storeRef(fin_cells[cell.ref2]);
            if (cell.ref3 != 0) {
              b.storeRef(fin_cells[cell.ref3]);
            }
          }
        }
        fin_cells.push(b.toCell());
      } else {
        fin_cells.push(cell.data);
      }
      cursor = cursor.loadRef().toSlice();
    }
    delete raw_cells;
    return fin_cells.pop();
  }
}
