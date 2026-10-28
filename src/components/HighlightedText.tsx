import { Fragment } from "react";
import { matchRanges } from "../conversationText";

export default function HighlightedText({
  text,
  query,
}: {
  text: string;
  query: string;
}) {
  const ranges = matchRanges(text, query);
  if (ranges.length === 0) return <>{text}</>;
  let previous = 0;
  const parts = ranges.map(([start, end]) => {
    const before = text.slice(previous, start);
    previous = end;
    return (
      <Fragment key={start}>
        {before}
        <mark className="search-highlight">{text.slice(start, end)}</mark>
      </Fragment>
    );
  });
  return (
    <>
      {parts}
      {text.slice(previous)}
    </>
  );
}
