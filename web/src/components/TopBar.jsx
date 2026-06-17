import React from 'react';
import { useStore } from '../store.js';

export default function TopBar() {
  const { openlrString, preset, showParams, showTrace, showSegmentLayer, decoding, decodeResult,
          setOpenlrString, applyPreset, toggleParams, toggleTrace, toggleSegmentLayer, runDecode } = useStore();

  return (
    <div className="top-bar">
      <input
        className="openlr-input"
        type="text"
        placeholder="Paste OpenLR string (v3 base64 or TPEG hex)…"
        value={openlrString}
        onChange={e => setOpenlrString(e.target.value)}
        onKeyDown={e => e.key === 'Enter' && runDecode()}
        spellCheck={false}
      />
      <select className="preset-select" value={preset} onChange={e => applyPreset(e.target.value)}>
        <option value="Permissive">Permissive</option>
        <option value="Default">Default</option>
        <option value="Strict">Strict</option>
      </select>
      <button
        className={`params-btn${showSegmentLayer ? ' active' : ''}`}
        onClick={toggleSegmentLayer}
        title={showSegmentLayer ? 'Hide all road segments' : 'Show all road segments (FRC colored, clickable)'}
      >{showSegmentLayer ? '● Segs' : '○ Segs'}</button>
      <button
        className={`params-btn${showParams ? ' active' : ''}`}
        onClick={toggleParams}
        title="Decode parameters"
      >⚙</button>
      <button
        className={`params-btn${showTrace ? ' active' : ''}`}
        onClick={toggleTrace}
        title="Decode trace"
      >⚡</button>
      <button className="decode-btn" onClick={runDecode} disabled={decoding}>
        {decoding ? '…' : 'Decode'}
      </button>
    </div>
  );
}
