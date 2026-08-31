import { badgeClass } from "../core/lib";
import type { PrFile } from "../core/github";

interface SidebarProps {
  files: PrFile[];
  onNavigate: () => void;
}

// Flat file list (phase 1 parity). Each item is an anchor to its `#file-{i}`
// card; the status colors the left border.
export function Sidebar({ files, onNavigate }: SidebarProps) {
  return (
    <nav className="sidebar">
      {files.map((file, i) => (
        <a
          key={i}
          href={`#file-${i}`}
          className={`side-item ${badgeClass(file.status)}`}
          title={file.filename}
          onClick={onNavigate}
        >
          {file.filename}
        </a>
      ))}
    </nav>
  );
}
