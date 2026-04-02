import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";

interface LandingProps {
  myKey: string;
  status: string;
  onCreateRoom: (name: string) => void;
  onJoinRoom: (name: string, hostKey: string) => void;
}

export function Landing({ myKey, status, onCreateRoom, onJoinRoom }: LandingProps) {
  const [name, setName] = useState("");
  const [hostKey, setHostKey] = useState("");
  const busy = status === "creating" || status === "joining";

  return (
    <div className="flex-1 flex items-center justify-center p-6">
      <div className="w-full max-w-md space-y-6">
        <div className="space-y-2 text-center">
          <h2 className="text-2xl font-semibold tracking-tight">Get Started</h2>
          <p className="text-sm text-muted-foreground">
            Create a new room or join an existing one
          </p>
        </div>

        <div className="space-y-2">
          <label className="text-sm font-medium">Your Name</label>
          <Input
            placeholder="Enter your name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>

        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-base">Create Room</CardTitle>
            <CardDescription>Host a chat room on this browser</CardDescription>
          </CardHeader>
          <CardContent>
            <Button
              className="w-full"
              disabled={busy}
              onClick={() => onCreateRoom(name || "Host")}
            >
              {status === "creating" ? "Creating..." : "Create Room"}
            </Button>
          </CardContent>
        </Card>

        <div className="relative">
          <div className="absolute inset-0 flex items-center">
            <Separator />
          </div>
          <div className="relative flex justify-center text-xs uppercase">
            <span className="bg-background px-2 text-muted-foreground">or</span>
          </div>
        </div>

        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-base">Join Room</CardTitle>
            <CardDescription>Connect to someone else's room</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Input
              placeholder="Paste host's public key"
              value={hostKey}
              onChange={(e) => setHostKey(e.target.value)}
            />
            <Button
              variant="secondary"
              className="w-full"
              disabled={busy || !hostKey.trim()}
              onClick={() => onJoinRoom(name || "Guest", hostKey.trim())}
            >
              {status === "joining" ? "Joining..." : "Join Room"}
            </Button>
          </CardContent>
        </Card>

        <p className="text-sm text-muted-foreground text-center">
          {status === "loading"
            ? "Loading WASM..."
            : status === "ready"
              ? `Ready. Your key: ${myKey.slice(0, 12)}...`
              : status}
        </p>
      </div>
    </div>
  );
}
