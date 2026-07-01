import React, { useState, useEffect } from 'react';
import { createPortal } from 'react-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

function DiagramBlock({ svg, index }) {
  const [copied,   setCopied]   = useState(false);
  const [expanded, setExpanded] = useState(false);

  const dataUrl = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;

  useEffect(() => {
    if (!expanded) return;
    const onKey = (e) => { if (e.key === 'Escape') setExpanded(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [expanded]);

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
    <>
      <div className="llm-diagram">
        <img
          src={dataUrl}
          alt="Diagram"
          className="llm-diagram-img llm-diagram-img--zoom"
          onClick={() => setExpanded(true)}
          title="Click to expand"
        />
        <div className="llm-diagram-actions">
          <button className="llm-diagram-btn" onClick={() => setExpanded(true)} title="Expand diagram">
            ⤢ Expand
          </button>
          <button className="llm-diagram-btn" onClick={copySvg} title="Copy SVG source">
            {copied ? '✓ Copied' : '⎘ SVG'}
          </button>
          <button className="llm-diagram-btn" onClick={exportPng} title="Download as PNG">
            ↓ PNG
          </button>
        </div>
      </div>

      {/* Portal to document.body so backdrop-filter on the chat panel doesn't trap the overlay */}
      {expanded && createPortal(
        <div className="llm-diagram-overlay" onClick={() => setExpanded(false)}>
          <img
            src={dataUrl}
            alt="Diagram (expanded)"
            className="llm-diagram-expanded"
            onClick={(e) => e.stopPropagation()}
          />
        </div>,
        document.body
      )}
    </>
  );
}

const MD_COMPONENTS = {
  a({ href, children }) {
    return <a href={href} target="_blank" rel="noopener noreferrer" className="llm-md-link">{children}</a>;
  },
  pre({ children }) {
    return <pre className="llm-pre">{children}</pre>;
  },
  code({ className, children }) {
    const text = String(children);
    if (className || text.includes('\n')) {
      return <code className={`llm-code-block-inner${className ? ` ${className}` : ''}`}>{text.replace(/\n$/, '')}</code>;
    }
    return <code className="llm-inline-code">{children}</code>;
  },
};

const DIAGRAM_RE = /(<diagram>[\s\S]*?<\/diagram>)/;

// streaming=true: if there is an unclosed <diagram> tag, hide everything from
// it onwards and show a spinner placeholder instead. Markdown text before it
// renders normally.
export function renderLlmText(text, { streaming = false } = {}) {
  if (!text) return null;

  let displayText = text;
  let pendingDiagram = false;

  if (streaming) {
    const openIdx = text.lastIndexOf('<diagram>');
    if (openIdx !== -1 && text.indexOf('</diagram>', openIdx) === -1) {
      displayText = text.slice(0, openIdx);
      pendingDiagram = true;
    }
  }

  const parts = displayText.split(DIAGRAM_RE);
  let diagIdx = 0;

  const elements = parts.map((part, i) => {
    if (part.startsWith('<diagram>') && part.endsWith('</diagram>')) {
      const svg = part.slice('<diagram>'.length, -'</diagram>'.length).trim();
      return <DiagramBlock key={`diag-${i}`} svg={svg} index={diagIdx++} />;
    }
    if (!part.trim()) return null;
    return (
      <div key={`md-${i}`} className="llm-md">
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>
          {part}
        </ReactMarkdown>
      </div>
    );
  }).filter(Boolean);

  if (pendingDiagram) {
    elements.push(
      <div key="pending-diagram" className="llm-diagram-pending">
        ⟳ diagram…
      </div>
    );
  }

  return elements.length ? elements : null;
}
