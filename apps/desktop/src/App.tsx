import { useEffect } from "react";
import { onScanEvents } from "./api";
import { useHoard } from "./store";
import { HomeScreen } from "./components/HomeScreen";
import { ScanningScreen } from "./components/ScanningScreen";
import { ResultsScreen } from "./components/ResultsScreen";
import { ContextMenu } from "./components/ContextMenu";

export function App() {
  const screen = useHoard((s) => s.screen);
  const toast = useHoard((s) => s.toast);

  useEffect(() => {
    const s = useHoard.getState();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onScanEvents({ progress: s.onProgress, batch: s.onBatch, done: s.onDone }).then((off) => {
      if (disposed) off();
      else unlisten = off;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return (
    <>
      {screen === "home" && <HomeScreen />}
      {screen === "scanning" && <ScanningScreen />}
      {screen === "results" && <ResultsScreen />}
      <ContextMenu />
      {toast && (
        <div className="toast" role="status">
          {toast}
        </div>
      )}
    </>
  );
}
