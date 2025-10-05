import React, { Component, ErrorInfo, ReactNode } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error?: Error;
  errorInfo?: ErrorInfo;
}

class ErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false
  };

  public static getDerivedStateFromError(error: Error): State {
    // Update state so the next render will show the fallback UI.
    return { hasError: true, error };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error('React Error Boundary caught an error:', error, errorInfo);
    
    // Log detailed error information
    console.group('🔴 React Error Details');
    console.error('Error:', error.name);
    console.error('Message:', error.message);
    console.error('Stack:', error.stack);
    console.error('Component Stack:', errorInfo.componentStack);
    console.groupEnd();
    
    // Store error details in state
    this.setState({
      error,
      errorInfo
    });
  }

  public render() {
    if (this.state.hasError) {
      return (
        <div style={{ 
          padding: '20px', 
          border: '1px solid red', 
          backgroundColor: '#fee', 
          margin: '20px',
          fontFamily: 'monospace'
        }}>
          <h2>⚠️ Something went wrong</h2>
          <details style={{ whiteSpace: 'pre-wrap' }}>
            <summary>Click to see error details</summary>
            <div>
              <h3>Error:</h3>
              <p>{this.state.error?.name}: {this.state.error?.message}</p>
              <h3>Stack Trace:</h3>
              <pre>{this.state.error?.stack}</pre>
              <h3>Component Stack:</h3>
              <pre>{this.state.errorInfo?.componentStack}</pre>
            </div>
          </details>
        </div>
      );
    }

    return this.props.children;
  }
}

export default ErrorBoundary;
