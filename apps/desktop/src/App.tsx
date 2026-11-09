import { useEffect } from "react";
import { onCleanupProgress, onScanEvents } from "./api";
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
    const offs: (() => void)[] = [];
    let disposed = false;
    const keep = (off: () => void) => {
      if (disposed) off();
      else offs.push(off);
    };
    void onScanEvents({ progress: s.onProgress, batch: s.onBatch, done: s.onDone }).then(keep);
    void onCleanupProgress(s.onCleanupProgress).then(keep);
    return () => {
      disposed = true;
      offs.forEach((off) => off());
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
