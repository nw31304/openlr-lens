import React, { useState } from 'react';

function renderInline(text) {
  const parts = text.split(/(\*\*[^*]+\*\*)/g);
  return parts.map((p, i) =>
    p.startsWith('**') && p.endsWith('**')
      ? <strong key={i}>{p.slice(2, -2)}</strong>
      : p
  );
}

function renderLine(line, key) {
  const trimmed = line.trimStart();
  if (trimmed.startsWith('- ') || trimmed.startsWith('• ')) {
    return <div key={key} className="llm-bullet">{renderInline(trimmed.slice(2))}</div>;
  }
  if (/^[A-Z][^:]{0,30}:\s*$/.test(trimmed)) {
    return <div key={key} className="llm-section-hdr">{trimmed.replace(/:$/, '')}</div>;
  }
  if (!trimmed) return <div key={key} className="llm-spacer" />;
  return <div key={key}>{renderInline(line)}</div>;
}

function DiagramBlock({ svg, index }) {
  const [copied, setCopied] = useState(false);

  const dataUrl = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;

  const copySvg = () => {
    navigator.clipboard.writeText(svg).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const exportPng = () => {
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement('canvas');
      canvas.width  = img.naturalWidth  || 600;
      canvas.height = img.naturalHeight || 300;
      canvas.getContext('2d').drawImage(img, 0, 0);
      canvas.toBlob(blob => {
        const a = document.createElement('a');
        a.href = URL.createObjectURL(blob);
        a.download = `diagram-${index + 1}.png`;
        a.click();
        setTimeout(() => URL.revokeObjectURL(a.href), 5000);
      });
    };
    img.src = dataUrl;
  };

  return (
    <div className="llm-diagram">
      <img src={dataUrl} alt="Diagram" className="llm-diagram-img" />
      <div className="llm-diagram-actions">
        <button className="llm-diagram-btn" onClick={copySvg} title="Copy SVG source">
          {copied ? '✓ Copied' : '⎘ SVG'}
        </button>
        <button className="llm-diagram-btn" onClick={exportPng} title="Download as PNG">
          ↓ PNG
        </button>
      </div>
    </div>
  );
}

export function renderLlmText(text) {
  if (!text) return null;

  // Split on <diagram>...</diagram> blocks (may be multiline)
  const parts = text.split(/(<diagram>[\s\S]*?<\/diagram>)/);
  let diagIdx = 0;

  return parts.flatMap((part, i) => {
    if (part.startsWith('<diagram>') && part.endsWith('</diagram>')) {
      const svg = part.slice('<diagram>'.length, -'</diagram>'.length).trim();
      return [<DiagramBlock key={`diag-${i}`} svg={svg} index={diagIdx++} />];
    }
    return part.split('\n').map((line, j) => renderLine(line, `${i}-${j}`));
  });
}
