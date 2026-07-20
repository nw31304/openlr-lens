import React from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useStore } from '../store.js';
import { HELP } from '../refFormat.js';
import userGuideMd from '../docs/userGuide.md?raw';

// Parameter reference is rendered from the *same* HELP object that powers
// the `?` tooltips in ResultPanel's Reference section, rather than
// duplicated as hand-written prose here -- one source of truth, so the
// tooltip and this doc can't silently drift apart the way two independent
// descriptions of the same field eventually would.
const PARAM_ORDER = ['frc', 'fow', 'bearing', 'dnp', 'lfrcnp', 'offset'];
const PARAM_TITLE = {
  frc: 'FRC (Functional Road Class)', fow: 'FOW (Form of Way)', bearing: 'Bearing',
  dnp: 'DNP (Distance to Next Point)', lfrcnp: 'LFRCNP', offset: 'Positive / Negative offset',
};

export default function DocumentationPanel() {
  const { showDocs, closeDocs } = useStore();
  if (!showDocs) return null;

  return (
    <div className="docs-panel">
      <div className="docs-panel-header">
        <span className="docs-panel-title">Documentation</span>
        <button className="docs-panel-close" onClick={closeDocs} title="Close">✕</button>
      </div>
      <div className="docs-panel-body">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{userGuideMd}</ReactMarkdown>
        <dl className="docs-param-list">
          {PARAM_ORDER.map(key => (
            <React.Fragment key={key}>
              <dt>{PARAM_TITLE[key]}</dt>
              <dd>{HELP[key]}</dd>
            </React.Fragment>
          ))}
        </dl>
      </div>
    </div>
  );
}
