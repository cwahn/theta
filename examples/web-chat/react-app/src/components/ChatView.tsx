import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import type { ChatMessage } from "@/hooks/use-chat";

interface ChatViewProps {
  role: "host" | "guest";
  peerKey: string;
  messages: ChatMessage[];
  onSend: (text: string) => void;
  author: string;
}

export function ChatView({ role, peerKey, messages, onSend, author }: ChatViewProps) {
  const [text, setText] = useState("");
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSend = () => {
    const trimmed = text.trim();
    if (!trimmed) return;
    onSend(trimmed);
    setText("");
  };

  const copyKey = async () => {
    await navigator.clipboard.writeText(peerKey);
  };

  return (
    <div className="flex-1 flex flex-col">
      <div className="border-b px-6 py-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Badge variant={role === "host" ? "default" : "secondary"}>
            {role === "host" ? "Hosting" : "Connected"}
          </Badge>
          <span
            className="text-xs text-muted-foreground font-mono cursor-pointer hover:text-foreground transition-colors"
            title="Click to copy"
            onClick={copyKey}
          >
            {peerKey.slice(0, 16)}...
          </span>
        </div>
        <span className="text-xs text-muted-foreground">Monitoring</span>
      </div>

      <ScrollArea className="flex-1 p-6">
        <div className="space-y-3">
          {messages.map((msg, i) => {
            const isMe = msg.author === author;
            const date = new Date(msg.timestamp * 1000);
            return (
              <div
                key={i}
                className={`rounded-lg border bg-card p-3 max-w-[80%] ${isMe ? "ml-auto" : ""}`}
              >
                <div className="flex items-baseline gap-2 mb-1">
                  <span className={`text-sm font-medium ${isMe ? "text-primary" : "text-muted-foreground"}`}>
                    {msg.author}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {date.toLocaleTimeString()}
                  </span>
                </div>
                <p className="text-sm">{msg.text}</p>
              </div>
            );
          })}
          <div ref={bottomRef} />
        </div>
      </ScrollArea>

      <div className="border-t p-4 flex gap-3">
        <Input
          className="flex-1"
          placeholder="Type a message..."
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSend()}
        />
        <Button onClick={handleSend}>Send</Button>
      </div>
    </div>
  );
}
