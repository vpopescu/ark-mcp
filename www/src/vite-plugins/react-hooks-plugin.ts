import type { Plugin } from 'vite';

export function reactHooksPlugin(): Plugin {
  return {
    name: 'react-hooks-polyfill',
    transform(code, _id) {
      // Patch any file that imports from react
      if (code.includes('from "react"') || code.includes("from 'react'")) {
        const polyfill = `
// Ensure React hooks are available
if (typeof React !== 'undefined' && !React.useLayoutEffect) {
  React.useLayoutEffect = React.useEffect;
}
`;
        return polyfill + code;
      }
      return null;
    },
    generateBundle(_options, bundle) {
      // Find the react-vendor chunk and prepend our polyfill
      for (const [fileName, chunk] of Object.entries(bundle)) {
        if (chunk.type === 'chunk' && fileName.includes('react-vendor')) {
          // Prepend React polyfill to the React vendor chunk
          const polyfill = `
// React hooks polyfill - fixes useLayoutEffect undefined errors
(function() {
  // Patch React namespace
  if (typeof exports !== 'undefined' && exports.useEffect && !exports.useLayoutEffect) {
    exports.useLayoutEffect = exports.useEffect;
  }
  
  // Patch global React
  if (typeof window !== 'undefined' && typeof React !== 'undefined') {
    if (!React.useLayoutEffect) {
      React.useLayoutEffect = React.useEffect || function() {};
    }
  }
})();
`;
          chunk.code = polyfill + chunk.code;
        }
      }
    }
  };
}
