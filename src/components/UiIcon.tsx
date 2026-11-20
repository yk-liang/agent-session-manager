import type { LucideIcon } from "lucide-react";

interface Props {
  icon: LucideIcon;
  size?: 12 | 14 | 16;
  className?: string;
}

/** 可访问名称由图标所属的控件提供。 */
export default function UiIcon({ icon: Graphic, size = 14, className = "" }: Props) {
  return <Graphic className={`ui-icon ${className}`.trim()} size={size} strokeWidth={1.75} aria-hidden="true" focusable="false" />;
}
